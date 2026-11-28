#![forbid(unsafe_code)]
//! Integration tests for M3.2 `retry_step("task" | "stage")` carry-forward and `card_runtime_sync`.

#[path = "workflow_board_m3_support/mod.rs"]
mod support;

use legion_persistence::project_store::{
    CreateProjectInput, create_project, ensure_project_store_schema,
};
use legion_persistence::sqlite::SqliteStore;
use legion_persistence::workflow_board_repo::{
    ClaimOrStartTaskInput, CompleteTaskInput, CreateCardInput, ResolveContractViolationInput,
    RetryStepInput, WorkflowBoardRepoError, claim_or_start_task, complete_task, create_card,
    get_board_snapshot, get_card_execution_detail, resolve_contract_violation, retry_step,
    set_card_worktree_if_unset, update_card_external_edit_state,
    update_card_product_snapshot_digest,
};
use support::setup_approved_execute_card;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn retry_step_task_stage_and_card_runtime_sync_helpers() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    ensure_project_store_schema(conn)?;
    create_project(
        conn,
        &CreateProjectInput::new("/tmp/proj-retry").with_project_id("proj-1"),
    )?;
    let t = "2026-11-24T02:00:00Z";

    create_card(
        conn,
        &CreateCardInput {
            card_id: "card-sync-wt",
            revision_id: "rev-sync-1",
            project_id: "proj-1",
            title: "Worktree Backfill Card",
            brief: "Test worktree backfill",
            intake_goal: None,
            brief_path: None,
            brief_digest: None,
            input_paths_json: None,
            source_type: "idea",
            assistance_mode: "direct",
            workspace_strategy_json: "{\"type\":\"worktree\"}",
            worktree_path: None,
            branch: None,
            model_selection_json: None,
            runtime_mode: None,
            interaction_mode: None,
            spec_json: "{\"acceptanceCriteria\":[{\"id\":\"AC-01\",\"text\":\"Tests pass\"}]}",
            spec_digest: Some("sha256:spec-sync"),
            wireframes_digest: None,
        },
        t,
    )?;

    assert!(set_card_worktree_if_unset(
        conn,
        "card-sync-wt",
        "/tmp/wt-1",
        Some("feat/card-sync-wt"),
        t
    )?);
    assert!(!set_card_worktree_if_unset(
        conn,
        "card-sync-wt",
        "/tmp/wt-other",
        Some("feat/other"),
        t
    )?);
    update_card_external_edit_state(conn, "card-sync-wt", "deliverable_stale", Some("edited"), t)?;
    update_card_product_snapshot_digest(conn, "card-sync-wt", "sha256:snap-sync", t)?;

    let snap = get_board_snapshot(conn, "proj-1", t)?;
    let summary = snap
        .cards
        .iter()
        .find(|c| c.card_id == "card-sync-wt")
        .ok_or("card summary missing")?;
    assert_eq!(
        summary.external_edit_state.as_deref(),
        Some("deliverable_stale")
    );
    let synced_detail = get_card_execution_detail(conn, "card-sync-wt")?;
    assert_eq!(
        synced_detail
            .planning
            .card_detail
            .card
            .worktree_path
            .as_deref(),
        Some("/tmp/wt-1")
    );
    Ok(())
}

#[test]
fn retry_step_task_and_stage_guards_clear_current_task() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    ensure_project_store_schema(conn)?;
    create_project(
        conn,
        &CreateProjectInput::new("/tmp/proj-retry-2").with_project_id("proj-1"),
    )?;
    let rev = setup_approved_execute_card(conn, "card-retry")?;
    let t = "2026-11-24T02:00:00Z";

    let started = claim_or_start_task(
        conn,
        &ClaimOrStartTaskInput {
            card_id: "card-retry",
            revision_id: "r-s1",
            expected_revision: rev,
            task_id: "TSK-1",
            stage_run_id: None,
        },
        t,
    )?;
    assert!(matches!(
        retry_step(
            conn,
            &RetryStepInput {
                card_id: "card-retry",
                revision_id: "r-bad-ip",
                expected_revision: started.planning.card_detail.card.revision,
                step_kind: "task",
                target_id: Some("TSK-1"),
                reason: None,
            },
            t,
        ),
        Err(WorkflowBoardRepoError::InvalidStateTransition(_))
    ));

    let failed_viol = complete_task(
        conn,
        &CompleteTaskInput {
            card_id: "card-retry",
            revision_id: "r-fail1",
            expected_revision: started.planning.card_detail.card.revision,
            task_id: "TSK-1",
            outcome_status: "failed",
            stage_run_id: None,
            thread_id: None,
            workflow_run_id: None,
            diff_digest: Some("sha256:d1"),
            product_snapshot_digest: Some("sha256:snap-sync"),
            error_message: Some("boom"),
            violation_id: None,
            changed_files: &["src/a.ts"],
            unexpected_files: &["src/unexp.ts"],
            forbidden_files: &[],
        },
        t,
    )?;
    resolve_violation_and_fail_clean(
        conn,
        failed_viol.planning.card_detail.card.revision,
        &failed_viol.contract_violations[0].id,
        t,
    )
}

fn resolve_violation_and_fail_clean(
    conn: &rusqlite::Connection,
    viol_rev: i64,
    viol_id: &str,
    t: &str,
) -> TestResult {
    assert!(matches!(
        retry_step(
            conn,
            &RetryStepInput {
                card_id: "card-retry",
                revision_id: "r-open-viol",
                expected_revision: viol_rev,
                step_kind: "task",
                target_id: Some("TSK-1"),
                reason: None,
            },
            t,
        ),
        Err(WorkflowBoardRepoError::PreconditionFailed(_))
    ));
    let resolved = resolve_contract_violation(
        conn,
        &ResolveContractViolationInput {
            card_id: "card-retry",
            revision_id: "r-resv",
            expected_revision: viol_rev,
            violation_id: viol_id,
            disposition: "manual_reverted",
            note: Some("Reverted unexpected edit"),
            resolved_by: "client",
        },
        t,
    )?;
    let started_fail = claim_or_start_task(
        conn,
        &ClaimOrStartTaskInput {
            card_id: "card-retry",
            revision_id: "r-s1b",
            expected_revision: resolved.planning.card_detail.card.revision,
            task_id: "TSK-1",
            stage_run_id: None,
        },
        t,
    )?;
    let failed_clean = complete_task(
        conn,
        &CompleteTaskInput {
            card_id: "card-retry",
            revision_id: "r-fail2",
            expected_revision: started_fail.planning.card_detail.card.revision,
            task_id: "TSK-1",
            outcome_status: "failed",
            stage_run_id: None,
            thread_id: None,
            workflow_run_id: None,
            diff_digest: Some("sha256:d2"),
            product_snapshot_digest: Some("sha256:snap-sync"),
            error_message: Some("test failure"),
            violation_id: None,
            changed_files: &["src/a.ts"],
            unexpected_files: &[],
            forbidden_files: &[],
        },
        t,
    )?;
    assert_task_and_stage_retry_clear_current_task(
        conn,
        failed_clean.planning.card_detail.card.revision,
        t,
    )
}

fn assert_task_and_stage_retry_clear_current_task(
    conn: &rusqlite::Connection,
    rev: i64,
    t: &str,
) -> TestResult {
    let retried_task = retry_step(
        conn,
        &RetryStepInput {
            card_id: "card-retry",
            revision_id: "r-ret-t1",
            expected_revision: rev,
            step_kind: "task",
            target_id: Some("TSK-1"),
            reason: Some("Retry after failure"),
        },
        t,
    )?;
    let t1 = retried_task
        .planning
        .tasks
        .iter()
        .find(|x| x.task_id == "TSK-1")
        .ok_or("TSK-1 missing")?;
    assert_eq!(t1.status, "todo");
    assert!(t1.active_stage_run_id.is_none());
    assert!(t1.started_at.is_none());
    assert!(t1.completed_at.is_none());
    assert!(
        retried_task
            .planning
            .card_detail
            .card
            .current_task_id
            .is_none()
    );

    let started2 = claim_or_start_task(
        conn,
        &ClaimOrStartTaskInput {
            card_id: "card-retry",
            revision_id: "r-s2",
            expected_revision: retried_task.planning.card_detail.card.revision,
            task_id: "TSK-1",
            stage_run_id: None,
        },
        t,
    )?;
    let stage_retried = retry_step(
        conn,
        &RetryStepInput {
            card_id: "card-retry",
            revision_id: "r-st-reset",
            expected_revision: started2.planning.card_detail.card.revision,
            step_kind: "stage",
            target_id: None,
            reason: Some("Stage reset"),
        },
        t,
    )?;
    let detail = get_card_execution_detail(conn, "card-retry")?;
    assert!(detail.planning.card_detail.card.current_task_id.is_none());
    assert_eq!(
        stage_retried
            .planning
            .tasks
            .iter()
            .find(|x| x.task_id == "TSK-1")
            .map(|x| x.status.as_str()),
        Some("todo")
    );
    Ok(())
}
