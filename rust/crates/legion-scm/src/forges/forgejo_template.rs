//! `Forgejo` Markdown pull-request template resolution matching root, `.forgejo/`,
//! `.gitea/`, `.github/`, and `docs/` blob entries (`#41095`).

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;

use crate::live_git_cli::execute_raw_bytes;

/// Maximum template bytes retained from a `Forgejo` PR template blob (`#41095`).
pub const FORGEJO_TEMPLATE_MAX_BYTES: usize = 8_000;

/// Maximum `git ls-tree` output byte limit when scanning template candidates (`#41095`).
pub const FORGEJO_TREE_LIST_MAX_BYTES: usize = 100_000;

/// Truncation suffix appended when a template exceeds [`FORGEJO_TEMPLATE_MAX_BYTES`] (`#41095`).
pub const FORGEJO_TEMPLATE_TRUNCATION_MARKER: &str = "[truncated]";

const TEMPLATE_DIRS: &[&str] = &["", ".forgejo/", ".gitea/", ".github/", "docs/"];
const TEMPLATE_FILES: &[&str] = &["PULL_REQUEST_TEMPLATE.md", "pull_request_template.md"];

fn ls_tree_blob_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let compiled = Regex::new(r"^100(?:644|755) blob ([0-9a-f]{40,64})\t(.+)$");
        // INFALLIBLE: constant `git ls-tree` regular blob matcher (`#41095`).
        compiled.expect("ls_tree_blob_regex must compile")
    })
}

/// Returns ordered candidate template relative paths following `Forgejo` and `Gitea` precedence (`#41095`).
#[must_use]
pub fn forgejo_template_paths() -> Vec<String> {
    let mut paths = Vec::with_capacity(TEMPLATE_DIRS.len() * TEMPLATE_FILES.len());
    for dir in TEMPLATE_DIRS {
        for file in TEMPLATE_FILES {
            paths.push(format!("{dir}{file}"));
        }
    }
    paths
}

/// Parses NUL-delimited `git ls-tree -z --full-tree` records, keeping ONLY regular blobs (`100644`/`100755`)
/// so symlinks (`120000`) are ignored (`#41095`).
#[must_use]
pub fn parse_forgejo_ls_tree_templates(ls_tree_z_stdout: &[u8]) -> BTreeMap<String, String> {
    let mut by_path = BTreeMap::new();
    let text = String::from_utf8_lossy(ls_tree_z_stdout);
    let re = ls_tree_blob_regex();
    for record in text.split('\0') {
        if let Some(caps) = re.captures(record)
            && let (Some(oid), Some(path)) = (caps.get(1), caps.get(2))
        {
            by_path.insert(path.as_str().to_owned(), oid.as_str().to_owned());
        }
    }
    by_path
}

/// Decodes and bounds a `Forgejo` PR template blob to [`FORGEJO_TEMPLATE_MAX_BYTES`] (`#41095`).
#[must_use]
pub fn bound_forgejo_pr_template(blob_bytes: &[u8]) -> Option<String> {
    let truncated = blob_bytes.len() > FORGEJO_TEMPLATE_MAX_BYTES;
    let slice = if truncated {
        let mut end = FORGEJO_TEMPLATE_MAX_BYTES;
        while end > 0 && std::str::from_utf8(&blob_bytes[..end]).is_err() {
            end -= 1;
        }
        &blob_bytes[..end]
    } else {
        blob_bytes
    };
    let text = String::from_utf8_lossy(slice).trim().to_owned();
    if text.is_empty() {
        return None;
    }
    if truncated && !text.ends_with(FORGEJO_TEMPLATE_TRUNCATION_MARKER) {
        Some(format!("{text}\n\n{FORGEJO_TEMPLATE_TRUNCATION_MARKER}"))
    } else {
        Some(text)
    }
}

/// Inspects `treeish` inside `repo_dir` via `git ls-tree` + `git cat-file blob` to detect the
/// highest-priority non-empty `Forgejo` Markdown pull request template (`#41095`).
pub async fn detect_forgejo_pr_template(
    repo_dir: &Path,
    treeish: &str,
    base_env: &[(String, String)],
) -> Option<String> {
    let clean_tree = treeish.trim();
    if clean_tree.is_empty() || clean_tree.starts_with('-') || clean_tree.contains('\0') {
        return None;
    }
    let candidates = forgejo_template_paths();
    let mut args = vec![
        "ls-tree".to_owned(),
        "-z".to_owned(),
        "--full-tree".to_owned(),
        clean_tree.to_owned(),
        "--".to_owned(),
    ];
    args.extend(candidates.iter().cloned());

    let listing = execute_raw_bytes("git", &args, base_env, &[], None, Some(repo_dir))
        .await
        .ok()?;
    if !listing.success || listing.stdout.len() > FORGEJO_TREE_LIST_MAX_BYTES {
        return None;
    }
    let objects = parse_forgejo_ls_tree_templates(&listing.stdout);
    for path in candidates {
        let Some(object_id) = objects.get(&path) else {
            continue;
        };
        let blob_args = ["cat-file".to_owned(), "blob".to_owned(), object_id.clone()];
        let Ok(blob) =
            execute_raw_bytes("git", &blob_args, base_env, &[], None, Some(repo_dir)).await
        else {
            continue;
        };
        if !blob.success {
            continue;
        }
        if let Some(template) = bound_forgejo_pr_template(&blob.stdout) {
            return Some(template);
        }
    }
    None
}
