//! Durable SQLite repositories for Project Workflow Board cards, revisions, approvals,
//! stage runs, and deliverable write-journal recovery (`BUILD_MIGRATION_HEAD = 114`).

mod blocker_status;
pub mod board_queries;
pub mod card_runtime_sync;
pub mod cards;
pub mod challenge;
mod challenge_validation;
pub mod cli_import;
mod cli_import_helpers;
mod cli_import_policies;
pub mod execution_actions;
mod execution_check_sync;
pub mod execution_checks;
pub mod execution_lifecycle;
pub mod execution_queries;
mod execution_retry;
pub mod execution_review;
mod execution_review_resolve;
mod execution_task_complete;
pub mod execution_tasks;
pub mod execution_violations;
pub mod exploration;
pub mod invalidation;
mod invalidation_digests;
pub mod plan_approve;
pub mod plan_readiness;
pub mod plan_repo;
pub mod plan_return;
mod plan_task_writes;
pub mod plan_validation;
mod plan_writes;
pub mod planning_queries;
mod ship_acceptance;
pub mod ship_blockers;
pub mod ship_repo;
pub mod spec_approvals;
pub mod stage_runs;
pub mod stage_start;
pub mod types;
pub mod types_m2;
pub mod types_m3;
pub mod types_m4;
pub mod write_journal;

use rusqlite::Connection;
use thiserror::Error;

pub use board_queries::{get_board_snapshot, list_project_cards};
pub use card_runtime_sync::{
    PlanApprovalCardPreview, card_has_remaining_blocker, preview_plan_approval_card_state,
    set_card_worktree_if_unset, update_card_external_edit_state,
    update_card_product_snapshot_digest,
};
pub use cards::{create_card, get_card_detail};
pub use challenge::challenge_step;
pub use cli_import::{
    apply_cli_import_tx, find_cli_import_by_spec, get_cli_import_by_card, pick_available_card_id,
};
pub use cli_import_policies::acknowledge_card_policies;
pub use execution_actions::{
    propose_amendment, reconcile_interrupted_action, record_external_action,
};
pub use execution_checks::{invalidate_snapshot_bound_evidence, record_check_receipt};
pub use execution_lifecycle::{pause_card, resume_card, retry_step};
pub use execution_queries::get_card_execution_detail;
pub use execution_review::record_review_run;
pub use execution_review_resolve::resolve_finding;
pub use execution_task_complete::complete_task;
pub use execution_tasks::{
    claim_or_start_task, effective_execute_plan_approval, verify_task_claim_preconditions,
};
pub use execution_violations::resolve_contract_violation;
pub use exploration::explore_step;
pub use invalidation::seed_exploration_decisions;
pub use plan_readiness::validate_spec_approval_m2_readiness;
pub use plan_repo::{approve_plan, return_for_revision, update_plan};
pub use plan_return::invalidate_plan_approval;
pub use planning_queries::get_card_planning_detail;
pub use ship_acceptance::record_manual_acceptance;
pub use ship_blockers::compute_ship_blockers;
pub use ship_repo::approve_delivery;
pub use spec_approvals::{approve_spec, invalidate_spec_approval, update_spec};
pub use stage_runs::{
    get_latest_stage_run_for_thread, get_stage_run_by_workflow_run_id, link_stage_run_thread,
    list_stage_runs_for_card, upsert_or_get_stage_run_by_launch_key,
};
pub use stage_start::{
    find_reusable_stage_run, find_reusable_stage_run_for_task, next_stage_attempt,
    record_stage_started_cas,
};
pub use types::*;
pub use types_m2::*;
pub use types_m3::*;
pub use types_m4::*;
pub use write_journal::{
    commit_journal, list_incomplete_journal_entries, mark_file_written, rollback_journal,
    stage_write,
};

/// Errors produced by [`workflow_board_repo`](self) persistence operations.
#[derive(Debug, Error)]
pub enum WorkflowBoardRepoError {
    /// Underlying SQLite failure.
    #[error("workflow board SQLite error during '{operation}': {source}")]
    Sqlite {
        /// SQLite operation name.
        operation: &'static str,
        /// Underlying `rusqlite::Error`.
        #[source]
        source: rusqlite::Error,
    },
    /// JSON serialization or deserialization failure.
    #[error("workflow board JSON error during '{operation}': {source}")]
    Json {
        /// JSON operation name.
        operation: &'static str,
        /// Underlying `serde_json::Error`.
        #[source]
        source: serde_json::Error,
    },
    /// Requested card does not exist.
    #[error("workflow board card '{card_id}' was not found")]
    CardNotFound {
        /// Missing card identifier.
        card_id: String,
    },
    /// Requested stage run does not exist.
    #[error("workflow board stage run '{id}' was not found")]
    StageRunNotFound {
        /// Missing stage run identifier.
        id: String,
    },
    /// Requested write-journal entry does not exist.
    #[error("workflow board write-journal entry '{id}' was not found")]
    JournalEntryNotFound {
        /// Missing journal entry identifier.
        id: String,
    },
    /// Optimistic concurrency check failed (`expected_revision != actual_revision`).
    #[error(
        "revision conflict on card '{card_id}': expected {expected_revision}, actual {actual_revision}"
    )]
    RevisionConflict {
        /// Card identifier.
        card_id: String,
        /// Caller-supplied expected revision.
        expected_revision: i64,
        /// Authoritative current revision in SQLite.
        actual_revision: i64,
    },
    /// Domain precondition failed (e.g. approving without a non-empty spec digest).
    #[error("workflow board precondition failed: {0}")]
    PreconditionFailed(String),
    /// Stable launch key already exists for a different card or stage.
    #[error("workflow board launch key reuse conflict: {0}")]
    ReuseConflict(String),
    /// Caller supplied invalid input.
    #[error("invalid workflow board input: {0}")]
    InvalidInput(String),
    /// Invalid write-journal or lifecycle state transition.
    #[error("invalid workflow board state transition: {0}")]
    InvalidStateTransition(String),
}

pub(crate) struct WriteTx<'a> {
    conn: &'a Connection,
    savepoint: bool,
    committed: bool,
}

impl<'a> WriteTx<'a> {
    pub(crate) fn begin(conn: &'a Connection) -> Result<Self, WorkflowBoardRepoError> {
        let savepoint = !conn.is_autocommit();
        let sql = if savepoint {
            "SAVEPOINT legion_workflow_board_repo;"
        } else {
            "BEGIN IMMEDIATE;"
        };
        conn.execute_batch(sql)
            .map_err(|source| WorkflowBoardRepoError::Sqlite {
                operation: "begin_transaction",
                source,
            })?;
        Ok(Self {
            conn,
            savepoint,
            committed: false,
        })
    }

    pub(crate) fn commit(mut self) -> Result<(), WorkflowBoardRepoError> {
        let sql = if self.savepoint {
            "RELEASE SAVEPOINT legion_workflow_board_repo;"
        } else {
            "COMMIT;"
        };
        self.conn
            .execute_batch(sql)
            .map_err(|source| WorkflowBoardRepoError::Sqlite {
                operation: "commit_transaction",
                source,
            })?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for WriteTx<'_> {
    fn drop(&mut self) {
        if !self.committed {
            let sql = if self.savepoint {
                "ROLLBACK TO SAVEPOINT legion_workflow_board_repo; RELEASE SAVEPOINT legion_workflow_board_repo;"
            } else {
                "ROLLBACK;"
            };
            let _ = self.conn.execute_batch(sql);
        }
    }
}

/// Ensures all workflow board v111 and v112 tables, columns, and indexes exist on `conn`.
pub fn ensure_workflow_board_schema(conn: &Connection) -> Result<(), WorkflowBoardRepoError> {
    crate::sqlite::schema_workflow_board::create_workflow_board_tables(conn).map_err(|source| {
        WorkflowBoardRepoError::Sqlite {
            operation: "create_workflow_board_tables",
            source,
        }
    })?;
    crate::sqlite::schema_workflow_board::create_workflow_board_indexes(conn).map_err(
        |source| WorkflowBoardRepoError::Sqlite {
            operation: "create_workflow_board_indexes",
            source,
        },
    )?;
    crate::sqlite::schema_workflow_board_m2::apply_m2_schema(conn).map_err(|source| {
        WorkflowBoardRepoError::Sqlite {
            operation: "apply_workflow_board_m2_schema",
            source,
        }
    })?;
    crate::sqlite::schema_workflow_board_m3::apply_m3_schema(conn).map_err(|source| {
        WorkflowBoardRepoError::Sqlite {
            operation: "apply_workflow_board_m3_schema",
            source,
        }
    })?;
    crate::sqlite::schema_workflow_board_m4::apply_m4_schema(conn).map_err(|source| {
        WorkflowBoardRepoError::Sqlite {
            operation: "apply_workflow_board_m4_schema",
            source,
        }
    })?;
    Ok(())
}

pub(crate) fn require_non_empty<'a>(
    value: &'a str,
    field: &'static str,
) -> Result<&'a str, WorkflowBoardRepoError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(WorkflowBoardRepoError::InvalidInput(format!(
            "{field} cannot be empty"
        )));
    }
    Ok(trimmed)
}
