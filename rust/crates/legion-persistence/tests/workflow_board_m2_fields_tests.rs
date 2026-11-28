#![forbid(unsafe_code)]
//! Persistence tests for Migration 112 M2 card columns and stage-run thread/workflow lookups.

use legion_persistence::project_store::{
    CreateProjectInput, create_project, ensure_project_store_schema,
};
use legion_persistence::sqlite::SqliteStore;
use legion_persistence::workflow_board_repo::types::{
    CreateCardInput, LinkStageRunThreadInput, UpdateSpecInput, UpsertStageRunInput,
};
use legion_persistence::workflow_board_repo::{
    create_card, get_latest_stage_run_for_thread, get_stage_run_by_workflow_run_id,
    link_stage_run_thread, update_spec, upsert_or_get_stage_run_by_launch_key,
};
use tempfile::TempDir;

type TestResult = Result<(), Box<dyn std::error::Error>>;
const TS: &str = "2026-11-24T12:00:00Z";

#[test]
fn card_m2_columns_round_trip_through_create_and_update_spec() -> TestResult {
    let tmp = TempDir::new()?;
    let store = SqliteStore::open(tmp.path().join("state.sqlite"))?;
    ensure_project_store_schema(store.connection())?;
    create_project(
        store.connection(),
        &CreateProjectInput::new(tmp.path().to_string_lossy().as_ref())
            .with_project_id("proj-m2-fields")
            .with_title("M2 Fields"),
    )?;
    let conn = store.connection();

    let created = create_card(
        conn,
        &CreateCardInput {
            card_id: "card-f1",
            project_id: "proj-m2-fields",
            revision_id: "rev-f1-1",
            title: "Fields Card",
            brief: "Brief body",
            source_type: "file",
            assistance_mode: "balanced",
            intake_goal: Some("change"),
            brief_path: Some("docs/brief.md"),
            brief_digest: Some("sha256:brief1"),
            input_paths_json: Some("[\"docs/brief.md\",\"src/lib.rs\"]"),
            wireframes_digest: Some("sha256:wf1"),
            spec_json: "{\"acceptanceCriteria\":[{\"id\":\"AC-01\",\"text\":\"Works\"}]}",
            spec_digest: Some("sha256:spec1"),
            workspace_strategy_json: "{\"type\":\"root\"}",
            worktree_path: None,
            branch: None,
            model_selection_json: None,
            runtime_mode: Some("full-access"),
            interaction_mode: Some("default"),
        },
        TS,
    )?;

    assert_eq!(created.card.intake_goal.as_deref(), Some("change"));
    assert_eq!(created.card.brief_path.as_deref(), Some("docs/brief.md"));
    assert_eq!(created.card.brief_digest.as_deref(), Some("sha256:brief1"));
    assert_eq!(
        created.card.input_paths_json,
        "[\"docs/brief.md\",\"src/lib.rs\"]"
    );
    assert_eq!(
        created.card.wireframes_digest.as_deref(),
        Some("sha256:wf1")
    );

    let updated = update_spec(
        conn,
        &UpdateSpecInput {
            card_id: "card-f1",
            revision_id: "rev-f1-2",
            expected_revision: 1,
            title: None,
            brief: Some("Updated brief"),
            intake_goal: Some("audit"),
            input_paths_json: Some("[\"README.md\"]"),
            wireframes_digest: None,
            clear_wireframes_digest: true,
            spec_json: Some("{\"acceptanceCriteria\":[{\"id\":\"AC-01\",\"text\":\"Works\"}]}"),
            spec_digest: Some("sha256:spec2"),
        },
        TS,
    )?;
    assert_eq!(updated.card.intake_goal.as_deref(), Some("audit"));
    assert_eq!(updated.card.input_paths_json, "[\"README.md\"]");
    assert_eq!(updated.card.wireframes_digest, None);
    Ok(())
}

#[test]
fn stage_run_thread_and_workflow_run_lookups_return_expected_row() -> TestResult {
    let tmp = TempDir::new()?;
    let store = SqliteStore::open(tmp.path().join("state.sqlite"))?;
    ensure_project_store_schema(store.connection())?;
    create_project(
        store.connection(),
        &CreateProjectInput::new(tmp.path().to_string_lossy().as_ref())
            .with_project_id("proj-m2-sr")
            .with_title("M2 SR"),
    )?;
    let conn = store.connection();
    create_card(
        conn,
        &CreateCardInput {
            card_id: "card-sr1",
            project_id: "proj-m2-sr",
            revision_id: "rev-sr1",
            title: "SR Card",
            brief: "B",
            source_type: "idea",
            assistance_mode: "balanced",
            intake_goal: None,
            brief_path: None,
            brief_digest: None,
            input_paths_json: None,
            wireframes_digest: None,
            spec_json: "{}",
            spec_digest: None,
            workspace_strategy_json: "{\"type\":\"root\"}",
            worktree_path: None,
            branch: None,
            model_selection_json: None,
            runtime_mode: None,
            interaction_mode: None,
        },
        TS,
    )?;

    upsert_or_get_stage_run_by_launch_key(
        conn,
        &UpsertStageRunInput {
            id: "sr-spec-1",
            card_id: "card-sr1",
            stage: "spec",
            task_id: None,
            attempt: 1,
            stable_launch_key: "wb:card-sr1:spec:1",
            card_revision: 1,
            provider_instance_id: "devReplay",
            model: "dev-replay",
            model_options_json: None,
            runtime_mode: "full-access",
            interaction_mode: "default",
            workspace_strategy_json: "{\"type\":\"root\"}",
            worktree_path: None,
            branch: None,
            brief_digest: None,
        },
        TS,
    )?;
    link_stage_run_thread(
        conn,
        &LinkStageRunThreadInput {
            id: "sr-spec-1",
            status: "running",
            thread_id: Some("thr-spec-1"),
            workflow_run_id: Some("wf-run-1"),
            worktree_path: None,
            branch: None,
            last_error: None,
            ended_at: None,
        },
        TS,
    )?;

    let by_thread = get_latest_stage_run_for_thread(conn, "thr-spec-1")?
        .ok_or("expected stage run for thread")?;
    assert_eq!(by_thread.id, "sr-spec-1");
    let by_wf = get_stage_run_by_workflow_run_id(conn, "wf-run-1")?
        .ok_or("expected stage run for workflow_run_id")?;
    assert_eq!(by_wf.id, "sr-spec-1");
    assert!(get_latest_stage_run_for_thread(conn, "thr-other")?.is_none());
    Ok(())
}
