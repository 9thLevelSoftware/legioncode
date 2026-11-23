//! Gitignore-aware workspace file/directory traversal shared by listing and search handlers.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::process::Stdio;

use serde_json::{Value, json};
use tokio::process::Command;

use super::projects_files::is_git_segment;

const MAX_WALK_ENTRIES: usize = 15_000;

pub(crate) async fn collect_workspace_entries(root: &Path) -> (Vec<Value>, bool, usize) {
    if let Some(git_files) = git_list_entries(root).await {
        let mut dirs = BTreeSet::new();
        for rel in &git_files {
            let mut cur = Path::new(rel).parent();
            while let Some(p) = cur {
                let s = p.to_string_lossy().replace('\\', "/");
                if s.is_empty() || s == "." {
                    break;
                }
                dirs.insert(s);
                cur = p.parent();
            }
        }
        let mut out = Vec::with_capacity(dirs.len() + git_files.len());
        let mut truncated = false;
        for d in dirs {
            if out.len() >= MAX_WALK_ENTRIES {
                truncated = true;
                break;
            }
            out.push(make_entry(&d, "directory"));
        }
        for f in git_files {
            if out.len() >= MAX_WALK_ENTRIES {
                truncated = true;
                break;
            }
            out.push(make_entry(&f, "file"));
        }
        return (out, truncated, 0);
    }

    let mut out = Vec::new();
    let mut skipped = 0_usize;
    let mut truncated = false;
    walk_dir_recursive(root, root, &mut out, &mut skipped, &mut truncated);
    (out, truncated, skipped)
}

async fn git_list_entries(root: &Path) -> Option<Vec<String>> {
    let out = Command::new("git")
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let files: Vec<String> = out
        .stdout
        .split(|&b| b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).replace('\\', "/"))
        .filter(|rel| !rel.split('/').any(is_git_segment))
        .collect();
    Some(files)
}

fn walk_dir_recursive(
    root: &Path,
    dir: &Path,
    out: &mut Vec<Value>,
    skipped: &mut usize,
    truncated: &mut bool,
) {
    if *truncated {
        return;
    }
    let Ok(rd) = fs::read_dir(dir) else {
        *skipped += 1;
        return;
    };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        if out.len() >= MAX_WALK_ENTRIES {
            *truncated = true;
            return;
        }
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if is_git_segment(&name_str) || name_str == "node_modules" {
            continue;
        }
        let path = entry.path();
        let Ok(rel) = path.strip_prefix(root) else {
            continue;
        };
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        if path.is_dir() {
            out.push(make_entry(&rel_str, "directory"));
            walk_dir_recursive(root, &path, out, skipped, truncated);
        } else if path.is_file() {
            out.push(make_entry(&rel_str, "file"));
        }
    }
}

fn make_entry(rel: &str, kind: &str) -> Value {
    let parent = Path::new(rel)
        .parent()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .filter(|s| !s.is_empty() && s != ".");
    match parent {
        Some(p) => json!({ "path": rel, "kind": kind, "parentPath": p }),
        None => json!({ "path": rel, "kind": kind }),
    }
}
