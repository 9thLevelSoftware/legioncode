#![forbid(unsafe_code)]
//! Workspace path confinement, safe card/artifact key validation, and deliverable I/O for `.legion/workflows/<card-id>/`.

use std::fs;
use std::path::{Component, Path, PathBuf};

use legion_contracts::workflow_board::{
    WorkflowBoardRequestError, WorkflowBoardRequestErrorCode, is_valid_artifact_key,
    is_valid_input_path,
};
use legion_persistence::project_store::find_project_by_id_or_workspace;
use legion_persistence::workflow_board_repo::types::CardRow;
use legion_workflows::sha256_prefixed;
use rusqlite::Connection;

const MAX_BRIEF_FILE_BYTES: usize = 1024 * 1024;
const MAX_ENUM_DEPTH: usize = 4;

fn key_err(msg: impl Into<String>) -> WorkflowBoardRequestError {
    WorkflowBoardRequestError::new(WorkflowBoardRequestErrorCode::InvalidArtifactKey, msg)
}

fn not_found_err(msg: impl Into<String>) -> WorkflowBoardRequestError {
    WorkflowBoardRequestError::new(WorkflowBoardRequestErrorCode::ArtifactNotFound, msg)
}

fn invalid_input_err(msg: impl Into<String>) -> WorkflowBoardRequestError {
    WorkflowBoardRequestError::new(WorkflowBoardRequestErrorCode::InvalidInput, msg)
}

/// Returns `true` when `meta` is a symlink or Windows reparse point (e.g. directory junction).
pub(crate) fn is_symlink_or_reparse_point(meta: &fs::Metadata) -> bool {
    let is_symlink = meta.file_type().is_symlink();
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt as _;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        is_symlink || (meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT) != 0
    }
    #[cfg(not(windows))]
    {
        is_symlink
    }
}

/// Returns `true` when `seg` names `.git` metadata (case-insensitive, Windows short-name/dot/stream hardened).
pub(crate) fn is_git_segment(seg: &str) -> bool {
    let head = seg.split(':').next().unwrap_or("");
    let cleaned = head.trim_end_matches(['.', ' ']).to_ascii_uppercase();
    cleaned == ".GIT" || cleaned == "GIT~1"
}

/// Validates `card_id` is a single safe normal path component (`Component::Normal`, no traversal or `.git`).
pub(crate) fn validate_safe_card_id(raw: &str) -> Result<&str, WorkflowBoardRequestError> {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.contains('\0')
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed.contains(':')
        || is_git_segment(trimmed)
    {
        return Err(key_err(format!(
            "Invalid cardId `{trimmed}` for deliverable path"
        )));
    }
    let mut comps = Path::new(trimmed).components();
    match (comps.next(), comps.next()) {
        (Some(Component::Normal(_)), None) => Ok(trimmed),
        _ => Err(key_err(format!(
            "Invalid cardId `{trimmed}` for deliverable path"
        ))),
    }
}

/// Validates an artifact relative key strictly (plus `.git` segment rejection).
pub(crate) fn validate_artifact_key_strict(raw: &str) -> Result<&str, WorkflowBoardRequestError> {
    let trimmed = raw.trim();
    if !is_valid_artifact_key(trimmed) || trimmed.split('/').any(is_git_segment) {
        return Err(key_err(format!("Invalid artifact key `{trimmed}`")));
    }
    Ok(trimmed)
}

/// Rejects sensitive credential files (`.env*`, `*.pem`, `*.key`, `.git`) for input paths.
fn is_forbidden_secret_relpath(rel: &str) -> bool {
    if !is_valid_input_path(rel) {
        return true;
    }
    for seg in rel.split('/') {
        let lower = seg.trim().to_ascii_lowercase();
        let has_secret_ext = Path::new(&lower).extension().is_some_and(|ext| {
            ext.eq_ignore_ascii_case("pem")
                || ext.eq_ignore_ascii_case("key")
                || ext.eq_ignore_ascii_case("p12")
                || ext.eq_ignore_ascii_case("pfx")
        });
        if is_git_segment(seg) || lower == ".env" || lower.starts_with(".env.") || has_secret_ext {
            return true;
        }
    }
    false
}

/// Resolves the authoritative deliverable workspace root for `card`.
pub(crate) fn resolve_card_workspace_root(
    conn: &Connection,
    card: &CardRow,
) -> Result<PathBuf, WorkflowBoardRequestError> {
    if let Some(wt) = card
        .worktree_path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return Ok(PathBuf::from(wt));
    }
    let project = find_project_by_id_or_workspace(conn, &card.project_id)
        .map_err(|e| {
            WorkflowBoardRequestError::new(
                WorkflowBoardRequestErrorCode::InternalError,
                format!("Failed to resolve project `{}`: {e}", card.project_id),
            )
        })?
        .ok_or_else(|| {
            WorkflowBoardRequestError::new(
                WorkflowBoardRequestErrorCode::ProjectNotFound,
                format!("Project `{}` was not found", card.project_id),
            )
        })?;
    Ok(PathBuf::from(project.workspace_root))
}

fn canonicalize_workspace_root(
    workspace_root: &Path,
) -> Result<PathBuf, WorkflowBoardRequestError> {
    if !workspace_root.is_dir() {
        return Err(WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::ProjectNotFound,
            format!(
                "Card workspace root `{}` does not exist",
                workspace_root.display()
            ),
        ));
    }
    fs::canonicalize(workspace_root).map_err(|e| {
        WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::InternalError,
            format!(
                "Failed to canonicalize workspace root `{}`: {e}",
                workspace_root.display()
            ),
        )
    })
}

fn verify_prefix_if_exists(
    canonical_root: &Path,
    candidate: &Path,
    label: &str,
) -> Result<(), WorkflowBoardRequestError> {
    if let Ok(meta) = fs::symlink_metadata(candidate) {
        if is_symlink_or_reparse_point(&meta) {
            return Err(key_err(format!(
                "Deliverable path `{label}` must not be a symlink or directory junction"
            )));
        }
        if meta.is_dir() || meta.is_file() {
            let canonical = fs::canonicalize(candidate)
                .map_err(|e| key_err(format!("Failed to resolve `{label}`: {e}")))?;
            if canonical.strip_prefix(canonical_root).is_err() {
                return Err(key_err(format!(
                    "Deliverable path `{label}` escapes workspace root"
                )));
            }
        }
    }
    Ok(())
}

fn verify_card_dir_prefixes(
    workspace_root: &Path,
    card_id: &str,
) -> Result<(PathBuf, PathBuf), WorkflowBoardRequestError> {
    let safe_id = validate_safe_card_id(card_id)?;
    let canonical_root = canonicalize_workspace_root(workspace_root)?;
    let dot_legion = canonical_root.join(".legion");
    let workflows_dir = dot_legion.join("workflows");
    let card_dir = workflows_dir.join(safe_id);

    verify_prefix_if_exists(&canonical_root, &dot_legion, ".legion")?;
    verify_prefix_if_exists(&canonical_root, &workflows_dir, ".legion/workflows")?;
    verify_prefix_if_exists(&canonical_root, &card_dir, safe_id)?;
    Ok((canonical_root, card_dir))
}

fn canonicalize_confined_card_dir(
    canonical_root: &Path,
    raw_card_dir: &Path,
) -> Result<PathBuf, WorkflowBoardRequestError> {
    let canonical_card_dir = fs::canonicalize(raw_card_dir).map_err(|e| {
        key_err(format!(
            "Failed to canonicalize workflow card directory `{}`: {e}",
            raw_card_dir.display()
        ))
    })?;
    if canonical_card_dir.strip_prefix(canonical_root).is_err() {
        return Err(key_err("Workflow card directory escapes workspace root"));
    }
    Ok(canonical_card_dir)
}

/// Resolves `<workspace_root>/.legion/workflows/<card-id>` if it exists and is strictly confined.
pub(crate) fn resolve_confined_card_dir_if_exists(
    workspace_root: &Path,
    card_id: &str,
) -> Result<Option<PathBuf>, WorkflowBoardRequestError> {
    let (canonical_root, raw_card_dir) = verify_card_dir_prefixes(workspace_root, card_id)?;
    if !raw_card_dir.is_dir() {
        return Ok(None);
    }
    Ok(Some(canonicalize_confined_card_dir(
        &canonical_root,
        &raw_card_dir,
    )?))
}

/// Ensures `<workspace_root>/.legion/workflows/<card-id>` exists and is strictly confined inside `workspace_root`.
pub(crate) fn ensure_confined_card_dir(
    workspace_root: &Path,
    card_id: &str,
) -> Result<PathBuf, WorkflowBoardRequestError> {
    let (canonical_root, card_dir) = verify_card_dir_prefixes(workspace_root, card_id)?;
    fs::create_dir_all(&card_dir).map_err(|e| {
        WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::InternalError,
            format!(
                "Failed to create workflow card directory `{}`: {e}",
                card_dir.display()
            ),
        )
    })?;
    canonicalize_confined_card_dir(&canonical_root, &card_dir)
}

/// Resolves an existing deliverable file strictly inside `<workspace_root>/.legion/workflows/<card-id>/`.
pub(crate) fn resolve_confined_artifact_path(
    workspace_root: &Path,
    card_id: &str,
    key: &str,
) -> Result<PathBuf, WorkflowBoardRequestError> {
    let safe_id = validate_safe_card_id(card_id)?;
    let safe_key = validate_artifact_key_strict(key)?;
    let Some(canonical_card_dir) = resolve_confined_card_dir_if_exists(workspace_root, safe_id)?
    else {
        return Err(not_found_err(format!(
            "Artifact `{safe_key}` not found for card `{safe_id}`"
        )));
    };

    let mut candidate = canonical_card_dir.clone();
    for seg in safe_key.split('/') {
        candidate.push(seg);
        let meta = fs::symlink_metadata(&candidate).map_err(|_| {
            not_found_err(format!(
                "Artifact `{safe_key}` not found for card `{safe_id}`"
            ))
        })?;
        if is_symlink_or_reparse_point(&meta) {
            return Err(key_err(format!(
                "Artifact `{safe_key}` must not traverse a symlink or directory junction"
            )));
        }
    }
    let canonical_file = fs::canonicalize(&candidate).map_err(|_| {
        not_found_err(format!(
            "Artifact `{safe_key}` not found for card `{safe_id}`"
        ))
    })?;
    if canonical_file.strip_prefix(&canonical_card_dir).is_err() {
        return Err(key_err(format!(
            "Artifact `{safe_key}` resolves outside card deliverables directory"
        )));
    }
    if !canonical_file.is_file() {
        return Err(not_found_err(format!(
            "Artifact `{safe_key}` is not a regular file"
        )));
    }
    Ok(canonical_file)
}

/// Reads an intake file (`briefPath` / `inputPaths`) confined to `workspace_root`, returning `(content, sha256_prefixed)`.
pub(crate) fn read_confined_input_file(
    workspace_root: &Path,
    rel_path: &str,
) -> Result<(String, String), WorkflowBoardRequestError> {
    let safe_rel = validate_artifact_key_strict(rel_path)?;
    if is_forbidden_secret_relpath(safe_rel) {
        return Err(key_err(format!(
            "Refusing credential or metadata path `{safe_rel}` as input path"
        )));
    }
    let canonical_root = canonicalize_workspace_root(workspace_root)?;
    let mut candidate = canonical_root.clone();
    for seg in safe_rel.split('/') {
        candidate.push(seg);
        let meta = fs::symlink_metadata(&candidate)
            .map_err(|_| not_found_err(format!("Input file `{safe_rel}` not found")))?;
        if is_symlink_or_reparse_point(&meta) {
            return Err(key_err(format!(
                "Refusing symlink or directory junction in input path `{safe_rel}`"
            )));
        }
    }
    let canonical_file = fs::canonicalize(&candidate)
        .map_err(|_| not_found_err(format!("Input file `{safe_rel}` not found")))?;
    let canonical_rel = canonical_file
        .strip_prefix(&canonical_root)
        .map_err(|_| key_err(format!("Input file `{safe_rel}` escapes workspace root")))?;
    let canonical_rel_str = canonical_rel.to_string_lossy().replace('\\', "/");
    if is_forbidden_secret_relpath(&canonical_rel_str) {
        return Err(key_err(format!(
            "Refusing credential or metadata path `{safe_rel}` as input path"
        )));
    }
    if !canonical_file.is_file() {
        return Err(invalid_input_err(format!(
            "Input path `{safe_rel}` is not a regular file"
        )));
    }
    let bytes = fs::read(&canonical_file).map_err(|e| {
        WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::InternalError,
            format!("Failed to read input file `{safe_rel}`: {e}"),
        )
    })?;
    if bytes.len() > MAX_BRIEF_FILE_BYTES {
        return Err(invalid_input_err(format!(
            "Input file `{safe_rel}` exceeds {MAX_BRIEF_FILE_BYTES} bytes"
        )));
    }
    if bytes.contains(&0) {
        return Err(invalid_input_err(format!(
            "Input file `{safe_rel}` appears to be binary"
        )));
    }
    let digest = sha256_prefixed(&bytes);
    let content = String::from_utf8(bytes)
        .map_err(|_| invalid_input_err(format!("Input file `{safe_rel}` is not valid UTF-8")))?;
    Ok((content, digest))
}

/// Lists safe relative artifact keys inside `<workspace_root>/.legion/workflows/<card-id>/`.
pub(crate) fn enumerate_artifact_keys(workspace_root: &Path, card_id: &str) -> Vec<String> {
    let Ok(Some(card_dir)) = resolve_confined_card_dir_if_exists(workspace_root, card_id) else {
        return Vec::new();
    };
    let mut keys = Vec::new();
    collect_keys_recursive(&card_dir, &card_dir, 0, &mut keys);
    keys.sort();
    keys
}

fn collect_keys_recursive(base: &Path, dir: &Path, depth: usize, out: &mut Vec<String>) {
    if depth > MAX_ENUM_DEPTH {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if is_symlink_or_reparse_point(&meta) {
            continue;
        }
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if is_git_segment(&name_str) || name_str.ends_with(".tmp") || name_str.starts_with(".tmp-")
        {
            continue;
        }
        if meta.is_dir() {
            collect_keys_recursive(base, &path, depth + 1, out);
        } else if meta.is_file()
            && let Ok(rel) = path.strip_prefix(base)
        {
            let key = rel.to_string_lossy().replace('\\', "/");
            if is_valid_artifact_key(&key) {
                out.push(key);
            }
        }
    }
}
