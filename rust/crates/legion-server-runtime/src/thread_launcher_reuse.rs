//! Thread reuse & crash-recovery compatibility checks for [`ThreadLauncher`](crate::thread_launcher::ThreadLauncher).

use std::path::Path;

use legion_contracts::ids::{ProjectId, ThreadId};
use legion_contracts::orchestration::WorkspaceStrategy;
use legion_orchestrator::projections::{WireThreadDetailSnapshot, query_wire_thread_detail};
use legion_persistence::thread_launch_store::ThreadLaunchRecord;
use legion_persistence::worktree_lookup::normalize_project_path_for_comparison;
use rusqlite::{Connection, OptionalExtension as _, params};
use serde_json::Value;

use crate::thread_launcher_types::{LaunchThreadResult, build_resumed_result};
use crate::thread_launcher_workspace::ThreadLaunchError;

pub(crate) fn try_reuse_existing_conn(
    conn: &Connection,
    thread_id: &str,
    canonical_project_id: &ProjectId,
    strategy: &WorkspaceStrategy,
    expected_worktree_path: Option<&str>,
) -> Result<Option<LaunchThreadResult>, ThreadLaunchError> {
    let Some(detail) = validate_existing_thread_compatibility(
        conn,
        thread_id,
        canonical_project_id,
        strategy,
        expected_worktree_path,
    )?
    else {
        return Ok(None);
    };
    let branch = detail
        .projection
        .thread
        .get("branch")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let worktree_path = detail
        .projection
        .thread
        .get("worktreePath")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let tid =
        ThreadId::new(thread_id).map_err(|e| ThreadLaunchError::Persistence(e.to_string()))?;
    Ok(Some(build_resumed_result(
        tid,
        detail.projection,
        detail.snapshot_sequence,
        branch,
        worktree_path,
    )))
}

pub(crate) fn validate_setup_recovery_compatibility(
    rec: &ThreadLaunchRecord,
    strategy: &WorkspaceStrategy,
) -> Result<(), ThreadLaunchError> {
    match strategy {
        WorkspaceStrategy::Root { branch } => {
            if rec.worktree_path.is_some() {
                return Err(ThreadLaunchError::ReuseConflict(
                    "Recorded launch setup used a worktree, but retry requested root".to_owned(),
                ));
            }
            check_branch_match(rec.branch.as_deref(), branch.as_deref(), "Recorded launch")
        }
        WorkspaceStrategy::ExistingWorktree {
            worktree_path,
            branch,
        } => {
            let Some(rec_wt) = rec.worktree_path.as_deref() else {
                return Err(ThreadLaunchError::ReuseConflict(
                    "Recorded launch setup has no worktreePath".to_owned(),
                ));
            };
            if normalize_project_path_for_comparison(rec_wt)
                != normalize_project_path_for_comparison(worktree_path.trim())
            {
                return Err(ThreadLaunchError::ReuseConflict(
                    "Recorded worktreePath does not match requested existing_worktree".to_owned(),
                ));
            }
            if !Path::new(rec_wt).is_dir() {
                return Err(ThreadLaunchError::WorktreeNotFound(rec_wt.to_owned()));
            }
            check_branch_match(rec.branch.as_deref(), branch.as_deref(), "Recorded launch")
        }
        WorkspaceStrategy::Worktree { branch, .. } => {
            let Some(rec_wt) = rec.worktree_path.as_deref() else {
                return Err(ThreadLaunchError::ReuseConflict(
                    "Recorded worktree setup has no worktreePath".to_owned(),
                ));
            };
            if !Path::new(rec_wt).is_dir() {
                return Err(ThreadLaunchError::WorktreeNotFound(rec_wt.to_owned()));
            }
            check_branch_match(rec.branch.as_deref(), branch.as_deref(), "Recorded launch")
        }
    }
}

struct ExistingThreadRow {
    deleted_at: Option<String>,
    project_id: String,
    worktree_path: Option<String>,
    branch: Option<String>,
}

pub(crate) fn validate_existing_thread_compatibility(
    conn: &Connection,
    thread_id: &str,
    expected_project_id: &ProjectId,
    strategy: &WorkspaceStrategy,
    expected_worktree_path: Option<&str>,
) -> Result<Option<WireThreadDetailSnapshot>, ThreadLaunchError> {
    let row: Option<ExistingThreadRow> = conn
        .query_row(
            "SELECT deleted_at, project_id,
                    json_extract(payload_json, '$.worktreePath'),
                    json_extract(payload_json, '$.branch')
             FROM orchestration_v2_projection_threads
             WHERE thread_id = ?1 LIMIT 1",
            params![thread_id],
            |r| {
                Ok(ExistingThreadRow {
                    deleted_at: r.get(0)?,
                    project_id: r.get(1)?,
                    worktree_path: r.get(2)?,
                    branch: r.get(3)?,
                })
            },
        )
        .optional()
        .map_err(|e| ThreadLaunchError::Persistence(e.to_string()))?;

    let Some(ExistingThreadRow {
        deleted_at,
        project_id: actual_project_id,
        worktree_path: actual_wt,
        branch: actual_branch,
    }) = row
    else {
        return Ok(None);
    };
    if deleted_at.is_some() {
        return Err(ThreadLaunchError::ThreadDeleted(thread_id.to_owned()));
    }
    if actual_project_id != expected_project_id.as_str() {
        return Err(ThreadLaunchError::ReuseConflict(format!(
            "Thread `{thread_id}` belongs to project `{actual_project_id}`, not `{}`",
            expected_project_id.as_str()
        )));
    }
    match strategy {
        WorkspaceStrategy::Root { branch } => {
            if actual_wt.is_some() {
                return Err(ThreadLaunchError::ReuseConflict(format!(
                    "Thread `{thread_id}` uses a worktree, expected root workspace"
                )));
            }
            check_branch_match(actual_branch.as_deref(), branch.as_deref(), thread_id)?;
        }
        WorkspaceStrategy::ExistingWorktree {
            worktree_path,
            branch,
        } => {
            let Some(ref wt) = actual_wt else {
                return Err(ThreadLaunchError::ReuseConflict(format!(
                    "Thread `{thread_id}` has no worktreePath"
                )));
            };
            if normalize_project_path_for_comparison(wt)
                != normalize_project_path_for_comparison(worktree_path.trim())
            {
                return Err(ThreadLaunchError::ReuseConflict(format!(
                    "Thread `{thread_id}` worktreePath `{wt}` does not match `{worktree_path}`"
                )));
            }
            if !Path::new(wt).is_dir() {
                return Err(ThreadLaunchError::WorktreeNotFound(wt.clone()));
            }
            check_branch_match(actual_branch.as_deref(), branch.as_deref(), thread_id)?;
        }
        WorkspaceStrategy::Worktree { branch, .. } => {
            let Some(ref wt) = actual_wt else {
                return Err(ThreadLaunchError::ReuseConflict(format!(
                    "Thread `{thread_id}` has no worktreePath"
                )));
            };
            if let Some(exp_wt) = expected_worktree_path
                && normalize_project_path_for_comparison(wt)
                    != normalize_project_path_for_comparison(exp_wt)
            {
                return Err(ThreadLaunchError::ReuseConflict(format!(
                    "Thread `{thread_id}` worktreePath `{wt}` does not match recorded `{exp_wt}`"
                )));
            }
            if !Path::new(wt).is_dir() {
                return Err(ThreadLaunchError::WorktreeNotFound(wt.clone()));
            }
            check_branch_match(actual_branch.as_deref(), branch.as_deref(), thread_id)?;
        }
    }
    let detail = query_wire_thread_detail(conn, thread_id)
        .map_err(|e| ThreadLaunchError::Persistence(e.to_string()))?;
    Ok(Some(detail))
}

fn check_branch_match(
    actual: Option<&str>,
    expected: Option<&str>,
    label: &str,
) -> Result<(), ThreadLaunchError> {
    if let Some(b) = expected.map(str::trim).filter(|s| !s.is_empty())
        && actual != Some(b)
    {
        return Err(ThreadLaunchError::ReuseConflict(format!(
            "`{label}` branch {actual:?} does not match requested `{b}`"
        )));
    }
    Ok(())
}
