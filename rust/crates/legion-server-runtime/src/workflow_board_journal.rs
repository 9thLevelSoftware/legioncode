#![forbid(unsafe_code)]
//! Write-journaled deliverable persistence (`stage_write` -> `.tmp` -> DB commit -> rename -> `commit_journal`).

use std::fs;
use std::path::{Component, Path, PathBuf};

use legion_contracts::workflow_board::{WorkflowBoardRequestError, WorkflowBoardRequestErrorCode};
use legion_persistence::workflow_board_repo::types::StageWriteInput;
use legion_persistence::workflow_board_repo::{
    commit_journal, mark_file_written, rollback_journal, stage_write,
};
use legion_workflows::sha256_prefixed;
use rusqlite::Connection;
use uuid::Uuid;

use crate::workflow_board_fs::{is_symlink_or_reparse_point, validate_artifact_key_strict};
use crate::workflow_board_types::map_repo_err;

/// One deliverable file write tracked by `workflow_board_write_journal`.
#[derive(Debug, Clone)]
pub(crate) struct JournaledWriteSpec {
    pub key: String,
    pub bytes: Vec<u8>,
    pub expected_digest: String,
    pub revision: Option<i64>,
}

pub(crate) struct StagedEntry {
    pub journal_id: String,
    pub card_dir: PathBuf,
    pub target_path: PathBuf,
    pub staging_path: PathBuf,
}

fn journal_key_err(msg: impl Into<String>) -> WorkflowBoardRequestError {
    WorkflowBoardRequestError::new(WorkflowBoardRequestErrorCode::InvalidArtifactKey, msg)
}

fn target_path_for_key(card_dir: &Path, key: &str) -> Result<PathBuf, WorkflowBoardRequestError> {
    let safe = validate_artifact_key_strict(key)?;
    let mut out = card_dir.to_path_buf();
    for seg in safe.split('/') {
        out.push(seg);
    }
    Ok(out)
}

fn staging_path_for_key(card_dir: &Path, key: &str, journal_id: &str) -> PathBuf {
    let flat = key.replace('/', "_");
    card_dir.join(format!("{flat}.{journal_id}.tmp"))
}

pub(crate) fn ensure_confined_deliverable_target(
    card_dir: &Path,
    target_path: &Path,
    create_parents: bool,
) -> Result<PathBuf, WorkflowBoardRequestError> {
    let canonical_card_dir = fs::canonicalize(card_dir).map_err(|e| {
        WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::InternalError,
            format!("Failed to canonicalize card directory: {e}"),
        )
    })?;
    let rel = target_path
        .strip_prefix(card_dir)
        .or_else(|_| target_path.strip_prefix(&canonical_card_dir));
    #[cfg(windows)]
    let rel = rel.or_else(|e| {
        if let Some(plain) = canonical_card_dir
            .to_str()
            .and_then(|s| s.strip_prefix(r"\\?\"))
        {
            return target_path.strip_prefix(Path::new(plain));
        }
        Err(e)
    });
    let rel = rel.map_err(|_| journal_key_err("Deliverable path escapes card directory"))?;
    let mut segs = Vec::new();
    for comp in rel.components() {
        match comp {
            Component::Normal(s) => segs.push(s),
            _ => return Err(journal_key_err("Deliverable path contains invalid segment")),
        }
    }
    let Some((file_name, parent_segs)) = segs.split_last() else {
        return Err(journal_key_err("Deliverable path cannot be empty"));
    };

    let mut cur = canonical_card_dir.clone();
    for seg in parent_segs {
        cur.push(seg);
        match fs::symlink_metadata(&cur) {
            Ok(meta) => {
                if is_symlink_or_reparse_point(&meta) {
                    return Err(journal_key_err(format!(
                        "Deliverable directory `{}` must not be a symlink or directory junction",
                        cur.display()
                    )));
                }
                if !meta.is_dir() {
                    return Err(journal_key_err(format!(
                        "Deliverable parent `{}` is not a directory",
                        cur.display()
                    )));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if !create_parents {
                    return Ok(target_path.to_path_buf());
                }
                fs::create_dir(&cur).map_err(|err| {
                    WorkflowBoardRequestError::new(
                        WorkflowBoardRequestErrorCode::InternalError,
                        format!("Failed to create deliverable parent dir: {err}"),
                    )
                })?;
            }
            Err(e) => {
                return Err(WorkflowBoardRequestError::new(
                    WorkflowBoardRequestErrorCode::InternalError,
                    format!("Failed to inspect deliverable parent dir: {e}"),
                ));
            }
        }
    }

    let canonical_parent = fs::canonicalize(&cur)
        .map_err(|e| journal_key_err(format!("Failed to canonicalize deliverable parent: {e}")))?;
    if canonical_parent.strip_prefix(&canonical_card_dir).is_err() {
        return Err(journal_key_err(
            "Deliverable parent directory escapes card directory",
        ));
    }
    let leaf = canonical_parent.join(file_name);
    if let Ok(meta) = fs::symlink_metadata(&leaf) {
        if is_symlink_or_reparse_point(&meta) {
            return Err(journal_key_err(
                "Deliverable file must not be a symlink or directory junction",
            ));
        }
        if meta.is_dir() {
            return Err(journal_key_err(
                "Deliverable target path points to a directory",
            ));
        }
    }
    Ok(leaf)
}

fn read_digest_if_file(path: &Path) -> Option<String> {
    let meta = fs::symlink_metadata(path).ok()?;
    if is_symlink_or_reparse_point(&meta) || !meta.is_file() {
        return None;
    }
    fs::read(path).ok().map(|b| sha256_prefixed(&b))
}

pub(crate) fn read_confined_digest_if_file(
    card_dir: &Path,
    target_path: &Path,
) -> Result<Option<String>, WorkflowBoardRequestError> {
    let safe_path = ensure_confined_deliverable_target(card_dir, target_path, false)?;
    Ok(read_digest_if_file(&safe_path))
}

pub(crate) fn read_confined_utf8_if_file(
    card_dir: &Path,
    target_path: &Path,
) -> Result<Option<String>, WorkflowBoardRequestError> {
    let safe_path = ensure_confined_deliverable_target(card_dir, target_path, false)?;
    let Ok(meta) = fs::symlink_metadata(&safe_path) else {
        return Ok(None);
    };
    if is_symlink_or_reparse_point(&meta) || !meta.is_file() {
        return Ok(None);
    }
    Ok(fs::read_to_string(&safe_path).ok())
}

fn is_confined_unlinked_card_dir(card_dir: &Path) -> bool {
    let mut cur = Some(card_dir);
    for _ in 0..3 {
        let Some(dir) = cur else {
            return false;
        };
        let Ok(meta) = fs::symlink_metadata(dir) else {
            return false;
        };
        if is_symlink_or_reparse_point(&meta) || !meta.is_dir() {
            return false;
        }
        cur = dir.parent();
    }
    true
}

pub(crate) fn remove_confined_staging_file(card_dir: &Path, staging_path: &Path) {
    if !is_confined_unlinked_card_dir(card_dir) {
        return;
    }
    let Ok(safe_st) = ensure_confined_deliverable_target(card_dir, staging_path, false) else {
        return;
    };
    if fs::symlink_metadata(&safe_st)
        .is_ok_and(|meta| !is_symlink_or_reparse_point(&meta) && meta.is_file())
    {
        let _ = fs::remove_file(&safe_st);
    }
}

fn cleanup_staged_failure(conn: &Connection, staged: &[StagedEntry], reason: &str, now_iso: &str) {
    for entry in staged {
        remove_confined_staging_file(&entry.card_dir, &entry.staging_path);
        let _ = rollback_journal(conn, &entry.journal_id, Some(reason), now_iso);
    }
}

pub(crate) fn stage_and_write_temp_files(
    conn: &Connection,
    card_id: &str,
    card_dir: &Path,
    writes: &[JournaledWriteSpec],
    now_iso: &str,
) -> Result<Vec<StagedEntry>, WorkflowBoardRequestError> {
    let mut staged = Vec::with_capacity(writes.len());
    for spec in writes {
        let raw_target = target_path_for_key(card_dir, &spec.key)?;
        let prev_digest = match read_confined_digest_if_file(card_dir, &raw_target) {
            Ok(dg) => dg,
            Err(err) => {
                cleanup_staged_failure(conn, &staged, &err.message, now_iso);
                return Err(err);
            }
        };
        let journal_id = format!("wj-{}", Uuid::new_v4().simple());
        let staging_path = staging_path_for_key(card_dir, &spec.key, &journal_id);
        let target_str = raw_target.to_string_lossy().into_owned();
        let staging_str = staging_path.to_string_lossy().into_owned();
        if let Err(err) = stage_write(
            conn,
            &StageWriteInput {
                id: &journal_id,
                card_id,
                target_path: &target_str,
                staging_path: Some(&staging_str),
                operation: "write",
                expected_digest: Some(&spec.expected_digest),
                previous_digest: prev_digest.as_deref(),
                revision: spec.revision,
            },
            now_iso,
        ) {
            let mapped = map_repo_err(err);
            cleanup_staged_failure(conn, &staged, &mapped.message, now_iso);
            return Err(mapped);
        }
        let safe_staging = match ensure_confined_deliverable_target(card_dir, &staging_path, false)
        {
            Ok(path) => path,
            Err(err) => {
                let _ = rollback_journal(conn, &journal_id, Some(&err.message), now_iso);
                cleanup_staged_failure(conn, &staged, &err.message, now_iso);
                return Err(err);
            }
        };
        if let Err(err) = fs::write(&safe_staging, &spec.bytes) {
            remove_confined_staging_file(card_dir, &safe_staging);
            let _ = rollback_journal(conn, &journal_id, Some(&err.to_string()), now_iso);
            cleanup_staged_failure(conn, &staged, &err.to_string(), now_iso);
            return Err(WorkflowBoardRequestError::new(
                WorkflowBoardRequestErrorCode::InternalError,
                format!(
                    "Failed to write staging deliverable `{}`: {err}",
                    staging_path.display()
                ),
            ));
        }
        staged.push(StagedEntry {
            journal_id,
            card_dir: card_dir.to_path_buf(),
            target_path: raw_target,
            staging_path,
        });
    }
    Ok(staged)
}

pub(crate) fn finalize_staged_files(
    conn: &Connection,
    staged: &[StagedEntry],
    now_iso: &str,
) -> Result<(), WorkflowBoardRequestError> {
    for entry in staged {
        if !is_confined_unlinked_card_dir(&entry.card_dir) {
            return Err(journal_key_err(
                "Card directory must not traverse a symlink or directory junction",
            ));
        }
        let safe_target =
            ensure_confined_deliverable_target(&entry.card_dir, &entry.target_path, true)?;
        if let Ok(meta) = fs::symlink_metadata(&entry.staging_path)
            && is_symlink_or_reparse_point(&meta)
        {
            return Err(journal_key_err(
                "Staging deliverable file must not be a symlink or directory junction",
            ));
        }
        fs::rename(&entry.staging_path, &safe_target).map_err(|e| {
            WorkflowBoardRequestError::new(
                WorkflowBoardRequestErrorCode::InternalError,
                format!(
                    "Failed to rename staged deliverable `{}` -> `{}`: {e}",
                    entry.staging_path.display(),
                    safe_target.display()
                ),
            )
        })?;
        mark_file_written(conn, &entry.journal_id, now_iso).map_err(map_repo_err)?;
        commit_journal(conn, &entry.journal_id, now_iso).map_err(map_repo_err)?;
    }
    Ok(())
}

/// Executes staged deliverable writes coordinated with an atomic SQLite commit closure.
pub(crate) fn execute_journaled_writes<F, R>(
    conn: &Connection,
    card_id: &str,
    card_dir: &Path,
    writes: &[JournaledWriteSpec],
    now_iso: &str,
    db_commit: F,
) -> Result<R, WorkflowBoardRequestError>
where
    F: FnOnce(&Connection) -> Result<R, WorkflowBoardRequestError>,
{
    let staged = stage_and_write_temp_files(conn, card_id, card_dir, writes, now_iso)?;
    let outcome = match db_commit(conn) {
        Ok(res) => res,
        Err(err) => {
            cleanup_staged_failure(conn, &staged, &err.message, now_iso);
            return Err(err);
        }
    };
    finalize_staged_files(conn, &staged, now_iso)?;
    Ok(outcome)
}

pub(crate) fn sweep_tmp_files(dir: &Path, depth: usize) -> usize {
    if depth > 4 {
        return 0;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if is_symlink_or_reparse_point(&meta) {
            continue;
        }
        if meta.is_dir() {
            removed += sweep_tmp_files(&path, depth + 1);
        } else if meta.is_file() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if (name_str.ends_with(".tmp") || name_str.starts_with(".tmp-"))
                && fs::remove_file(&path).is_ok()
            {
                removed += 1;
            }
        }
    }
    removed
}
