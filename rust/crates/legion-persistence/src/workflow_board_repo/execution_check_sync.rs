//! Automated AC evidence synchronization when snapshot-bound planned checks finish.

use rusqlite::{Connection, OptionalExtension as _, params};

use super::WorkflowBoardRepoError;

pub(crate) fn sync_automated_ac_evidence(
    conn: &Connection,
    card_id: &str,
    snapshot_digest: &str,
    now: &str,
) -> Result<(), WorkflowBoardRepoError> {
    let mut ac_stmt = conn
        .prepare(
            "SELECT criterion_id, check_ids_json
             FROM workflow_board_ac_evidence
             WHERE card_id = ?1 AND method = 'automated'",
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "prepare_automated_ac_sync",
            source,
        })?;
    let rows = ac_stmt
        .query_map(params![card_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "query_automated_ac_sync",
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "collect_automated_ac_sync",
            source,
        })?;

    for (criterion_id, check_ids_json) in rows {
        let check_ids: Vec<String> = serde_json::from_str(&check_ids_json).unwrap_or_default();
        if check_ids.is_empty() {
            continue;
        }
        let mut any_failed = false;
        let mut all_passed = true;
        for cid in &check_ids {
            let status: Option<String> = conn
                .query_row(
                    "SELECT status FROM workflow_board_planned_checks
                     WHERE card_id = ?1 AND check_id = ?2 AND COALESCE(product_snapshot_digest, '') = ?3",
                    params![card_id, cid, snapshot_digest],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|source| WorkflowBoardRepoError::Sqlite {
                    operation: "lookup_linked_check_status",
                    source,
                })?;
            match status.as_deref() {
                Some("passed") => {}
                Some("failed") => {
                    any_failed = true;
                    all_passed = false;
                }
                _ => all_passed = false,
            }
        }
        let next_ac_status = if any_failed {
            Some("fail")
        } else if all_passed {
            Some("pass")
        } else {
            None
        };
        if let Some(next_status) = next_ac_status {
            conn.execute(
                "UPDATE workflow_board_ac_evidence
                 SET status = ?3,
                     product_snapshot_digest = ?4,
                     stale_reason = NULL,
                     observed_by = 'automated_check',
                     observed_at = ?5,
                     updated_at = ?5
                 WHERE card_id = ?1 AND criterion_id = ?2",
                params![card_id, criterion_id, next_status, snapshot_digest, now],
            )
            .map_err(|source| WorkflowBoardRepoError::Sqlite {
                operation: "update_automated_ac_status",
                source,
            })?;
        }
    }
    Ok(())
}
