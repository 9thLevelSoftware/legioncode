#![forbid(unsafe_code)]
//! Unsupported CLI policy gating, historical plan approval check, and blocked-task Execute errors.

use std::fs;
use std::path::Path;

use legion_contracts::workflow_board::{
    WorkflowBoardCliImportPolicyStatus, WorkflowBoardCliImportUnsupportedPolicy,
    WorkflowBoardRequestError, WorkflowBoardRequestErrorCode, WorkflowBoardStartStageInput,
};
use legion_persistence::workflow_board_repo::acknowledge_card_policies;
use legion_persistence::workflow_board_repo::types_m2::TaskContractRow;
use legion_persistence::workflow_board_repo::types_m3::CardExecutionDetail;
use rusqlite::Connection;
use uuid::Uuid;

use crate::workflow_board_checks::sync_cli_policy_deliverables;
use crate::workflow_board_fs::{ensure_confined_card_dir, is_symlink_or_reparse_point};
use crate::workflow_board_types::map_repo_err;

/// Enforces or acknowledges unsupported adopted `.legion-cli` policies before Execute launch.
pub(crate) fn enforce_or_acknowledge_cli_policies(
    conn: &Connection,
    exec: &CardExecutionDetail,
    ws_root: &Path,
    card_id: &str,
    input: &WorkflowBoardStartStageInput,
    expected_rev_i64: i64,
    now_iso: &str,
) -> Result<Option<i64>, WorkflowBoardRequestError> {
    let Some(ref cli) = exec.cli_import else {
        return Ok(None);
    };
    let policies: Vec<WorkflowBoardCliImportUnsupportedPolicy> =
        serde_json::from_str(&cli.unsupported_policies_json).unwrap_or_default();
    let unacked_blocking: Vec<String> = policies
        .iter()
        .filter(|p| {
            p.blocks_execution && p.status != WorkflowBoardCliImportPolicyStatus::Acknowledged
        })
        .map(|p| p.key.clone())
        .collect();
    if unacked_blocking.is_empty() {
        return Ok(None);
    }

    if input.acknowledge_unsupported_policies != Some(true) {
        return Err(WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::InvalidStageTransition,
            format!(
                "Cannot start Execute while unsupported adopted CLI policies remain unacknowledged ({}); pass acknowledgeUnsupportedPolicies: true to acknowledge",
                unacked_blocking.join(", ")
            ),
        ));
    }

    let rev_id = format!("rev-ack-{}", Uuid::new_v4().simple());
    let (_, next_rev) = acknowledge_card_policies(
        conn,
        card_id,
        expected_rev_i64,
        &rev_id,
        input.acknowledged_policies.as_deref(),
        now_iso,
    )
    .map_err(map_repo_err)?;
    sync_cli_policy_deliverables(conn, ws_root, card_id, now_iso)?;
    Ok(Some(next_rev))
}

/// Surfaces blocked CLI root tasks (e.g. `TSK-0002`) explicitly when no dependency-ready task exists.
pub(crate) fn blocked_task_conflict_error(tasks: &[TaskContractRow]) -> WorkflowBoardRequestError {
    if let Some(blocked) = tasks.iter().find(|t| t.status == "blocked") {
        let reason = blocked
            .blocker_reason
            .as_deref()
            .or(blocked.last_error.as_deref())
            .unwrap_or("task is blocked");
        return WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::ContractConflict,
            format!(
                "No dependency-ready task is available to execute; task `{}` is `blocked` ({reason})",
                blocked.task_id
            ),
        );
    }
    WorkflowBoardRequestError::new(
        WorkflowBoardRequestErrorCode::ContractConflict,
        "No dependency-ready task is available to execute",
    )
}

pub(crate) fn copy_card_dir_if_new_worktree(
    src_card_dir: &Path,
    new_ws_root: &Path,
    card_id: &str,
) {
    let Ok(dst_card_dir) = ensure_confined_card_dir(new_ws_root, card_id) else {
        return;
    };
    if src_card_dir == dst_card_dir {
        return;
    }
    copy_dir_missing(src_card_dir, &dst_card_dir, 0);
}

fn copy_dir_missing(src: &Path, dst: &Path, depth: usize) {
    if depth > 3 {
        return;
    }
    let Ok(entries) = fs::read_dir(src) else {
        return;
    };
    for entry in entries.flatten() {
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let Ok(meta) = fs::symlink_metadata(&from) else {
            continue;
        };
        if is_symlink_or_reparse_point(&meta) {
            continue;
        }
        if meta.is_dir() {
            let _ = fs::create_dir_all(&to);
            copy_dir_missing(&from, &to, depth + 1);
        } else if meta.is_file() && !to.exists() {
            let _ = fs::copy(&from, &to);
        }
    }
}
