//! Live git checkpoint capture, diff persistence, and `BUG-02` guarded rollback.

use std::path::Path;

use legion_contracts::ids::ThreadId;
use legion_contracts::orchestration::CheckpointFileSummary;
use legion_scm::checkpoints::rollback::{
    RollbackCoordinator, RollbackIntent, VerifiedRollbackIntent,
};
use legion_scm::checkpoints::store::{CheckpointDiffBlob, CheckpointStore};
use rusqlite::Connection;

use crate::error::ServerRuntimeError;
use crate::git_checkpoints_cli::{
    build_checkpoint_ref, capture_snapshot_commit, compute_checkpoint_diff, execute_restore_to_ref,
    is_git_workspace, list_tracked_paths, resolve_commit_oid,
};

/// Request to capture a git checkpoint for a workspace and persist its diff blob.
#[derive(Debug, Clone)]
pub struct CaptureCheckpointRequest<'a> {
    /// Git repository working tree path.
    pub workspace_root: &'a Path,
    /// Owning thread identifier.
    pub thread_id: &'a ThreadId,
    /// Checkpoint scope identifier inserted into `refs/legioncode/.../<scope>/ordinal/<n>`.
    pub scope: &'a str,
    /// Ordinal within the scope.
    pub ordinal: u64,
    /// Parent checkpoint ref to diff from, if any.
    pub parent_ref: Option<&'a str>,
    /// Turn count lower bound stored in `checkpoint_diff_blobs`.
    pub from_turn_count: u32,
    /// Turn count upper bound stored in `checkpoint_diff_blobs`.
    pub to_turn_count: u32,
    /// ISO-8601 capture timestamp.
    pub captured_at: &'a str,
}

/// Output of a completed checkpoint capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedCheckpoint {
    /// Hidden git ref under `refs/legioncode/orchestration-v2/checkpoints/<scope>/ordinal/<n>`.
    pub git_ref: String,
    /// Resolved git commit SHA for the snapshot.
    pub commit_oid: String,
    /// File-level numstat summaries.
    pub files: Vec<CheckpointFileSummary>,
    /// Unified diff text persisted in [`CheckpointStore`].
    pub diff: String,
}

/// Outcome of a verified workspace rollback execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RollbackExecutionOutcome {
    /// Verified rollback intent enforced via [`RollbackCoordinator`].
    pub verified: VerifiedRollbackIntent,
    /// Target git checkpoint ref restored, if files were restored.
    pub target_ref: Option<String>,
    /// Commit OID resolved for `target_ref`, if files were restored.
    pub restored_commit_oid: Option<String>,
}

/// Engine bridging real `git` workspace operations with `legion-scm` persistence and guards.
#[derive(Debug, Default, Clone, Copy)]
pub struct GitCheckpointEngine;

impl GitCheckpointEngine {
    /// Create a stateless [`GitCheckpointEngine`].
    pub const fn new() -> Self {
        Self
    }

    /// Capture a workspace snapshot commit under `refs/legioncode/orchestration-v2/checkpoints/...`
    /// and save the resulting diff in [`CheckpointStore`].
    pub fn capture_checkpoint(
        &self,
        conn: &Connection,
        req: &CaptureCheckpointRequest<'_>,
    ) -> Result<CapturedCheckpoint, ServerRuntimeError> {
        let git_ref = build_checkpoint_ref(req.scope, req.ordinal)?;
        let message = format!(
            "legioncode checkpoint scope={} ordinal={}",
            req.scope, req.ordinal
        );
        let commit_oid = capture_snapshot_commit(req.workspace_root, &git_ref, &message)?;
        let (mut files, diff) =
            compute_checkpoint_diff(req.workspace_root, req.parent_ref, &commit_oid)?;
        annotate_file_change_kinds(req.workspace_root, req.parent_ref, &commit_oid, &mut files);

        CheckpointStore::new(conn).save_diff_blob(&CheckpointDiffBlob {
            thread_id: req.thread_id.clone(),
            from_turn_count: req.from_turn_count,
            to_turn_count: req.to_turn_count,
            diff: diff.clone(),
            created_at: req.captured_at.to_owned(),
        })?;

        Ok(CapturedCheckpoint {
            git_ref,
            commit_oid,
            files,
            diff,
        })
    }

    /// Execute a checkpoint rollback after enforcing `BUG-02` pending-intent verification.
    pub fn rollback_checked(
        &self,
        conn: &Connection,
        workspace_root: &Path,
        intent: &RollbackIntent,
        target_ref_override: Option<&str>,
    ) -> Result<RollbackExecutionOutcome, ServerRuntimeError> {
        let verified = RollbackCoordinator::verify_pending_intent(conn, intent)?;
        let target_ref = match target_ref_override {
            Some(r) => Some(r.to_owned()),
            None => lookup_checkpoint_ref(
                conn,
                intent.thread_id.as_str(),
                intent.checkpoint_id.as_str(),
            )?,
        };

        if !verified.restore_files() {
            return Ok(RollbackExecutionOutcome {
                verified,
                target_ref,
                restored_commit_oid: None,
            });
        }

        let resolved_ref = target_ref.clone().ok_or_else(|| {
            ServerRuntimeError::NotFound(format!(
                "missing checkpoint ref for `{}`",
                intent.checkpoint_id
            ))
        })?;

        if !is_git_workspace(workspace_root) {
            return Err(ServerRuntimeError::Conflict(format!(
                "`{}` is not a git work tree",
                workspace_root.display()
            )));
        }
        let tracked = list_tracked_paths(workspace_root)?;
        let tracked_refs: Vec<&str> = tracked.iter().map(String::as_str).collect();
        let plan = RollbackCoordinator::plan_git_restore(&verified, &tracked_refs, true);
        let restored_commit_oid = execute_restore_to_ref(workspace_root, &resolved_ref, &plan)?;

        Ok(RollbackExecutionOutcome {
            verified,
            target_ref,
            restored_commit_oid: Some(restored_commit_oid),
        })
    }

    /// Look up the `checkpoint.captured` git ref from `orchestration_events` for a thread & checkpoint.
    pub fn find_checkpoint_ref(
        &self,
        conn: &Connection,
        thread_id: &str,
        checkpoint_id: &str,
    ) -> Result<Option<String>, ServerRuntimeError> {
        lookup_checkpoint_ref(conn, thread_id, checkpoint_id)
    }
}

fn lookup_checkpoint_ref(
    conn: &Connection,
    thread_id: &str,
    checkpoint_id: &str,
) -> Result<Option<String>, ServerRuntimeError> {
    let mut stmt = conn.prepare(
        "SELECT json_extract(payload_json, '$.ref')
         FROM orchestration_events
         WHERE stream_id = ?1
           AND event_type = 'checkpoint.captured'
           AND json_extract(payload_json, '$.id') = ?2
         ORDER BY sequence DESC
         LIMIT 1",
    )?;
    let found = stmt.query_row(rusqlite::params![thread_id, checkpoint_id], |row| {
        row.get::<_, Option<String>>(0)
    });
    match found {
        Ok(val) => Ok(val),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(err) => Err(ServerRuntimeError::from(err)),
    }
}

fn annotate_file_change_kinds(
    workspace_root: &Path,
    parent_ref: Option<&str>,
    commit_oid: &str,
    files: &mut [CheckpointFileSummary],
) {
    let base = parent_ref.or_else(|| {
        resolve_commit_oid(workspace_root, "HEAD")
            .is_some()
            .then_some("HEAD")
    });
    let status_args = if let Some(b) = base {
        vec!["diff", "--name-status", b, commit_oid]
    } else {
        vec![
            "diff-tree",
            "--no-commit-id",
            "--name-status",
            "-r",
            "--root",
            commit_oid,
        ]
    };
    let Ok(bytes) = crate::git_checkpoints_cli::run_git(workspace_root, &status_args, &[]) else {
        return;
    };
    let status_text = String::from_utf8_lossy(&bytes);
    for line in status_text.lines() {
        let mut parts = line.split('\t');
        let Some(code) = parts.next() else { continue };
        let Some(path) = parts.next() else { continue };
        let kind = match code.chars().next() {
            Some('A') => "added",
            Some('D') => "deleted",
            Some('R') => "renamed",
            _ => "modified",
        };
        if let Some(entry) = files.iter_mut().find(|item| item.path == path) {
            kind.clone_into(&mut entry.kind);
        }
    }
}
