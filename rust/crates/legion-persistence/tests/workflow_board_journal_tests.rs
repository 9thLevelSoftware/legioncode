//! Tests for `workflow_board_repo` stage run idempotency, DB reopen durability, and write-journal recovery.

use legion_persistence::sqlite::SqliteStore;
use legion_persistence::workflow_board_repo::{
    ApproveSpecInput, CreateCardInput, LinkStageRunThreadInput, StageWriteInput,
    UpsertStageRunInput, UpsertStageRunOutcome, WorkflowBoardRepoError, approve_spec,
    commit_journal, create_card, get_card_detail, link_stage_run_thread,
    list_incomplete_journal_entries, list_stage_runs_for_card, mark_file_written, rollback_journal,
    stage_write, upsert_or_get_stage_run_by_launch_key,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn seed_approved_card(conn: &rusqlite::Connection, card_id: &str) -> TestResult {
    create_card(
        conn,
        &CreateCardInput {
            card_id,
            revision_id: Box::leak(format!("rev-{card_id}-1").into_boxed_str()),
            project_id: "proj-1",
            title: "Durable Card",
            brief: "Initial brief",
            intake_goal: None,
            brief_path: None,
            brief_digest: None,
            input_paths_json: None,
            source_type: "idea",
            assistance_mode: "balanced",
            workspace_strategy_json: "{\"type\":\"root\"}",
            worktree_path: None,
            branch: None,
            model_selection_json: Some("{\"instanceId\":\"devReplay\",\"model\":\"dev-replay\"}"),
            runtime_mode: Some("auto-accept-edits"),
            interaction_mode: Some("plan"),
            spec_json: r#"{"constraints":["No auto-commits"],"acceptanceCriteria":[{"id":"AC-1","text":"Journal commits survive reopen"}]}"#,
            spec_digest: Some("sha256:initial"),
            wireframes_digest: None,
        },
        "2026-11-24T00:00:00Z",
    )?;
    approve_spec(
        conn,
        &ApproveSpecInput {
            card_id,
            revision_id: Box::leak(format!("rev-{card_id}-2").into_boxed_str()),
            approval_id: Box::leak(format!("apr-{card_id}-1").into_boxed_str()),
            expected_revision: 1,
            expected_spec_digest: Some("sha256:initial"),
            wireframes_digest: None,
            approved_by: "client",
        },
        "2026-11-24T00:01:00Z",
    )?;
    Ok(())
}

fn sample_stage_run_input<'a>(
    id: &'a str,
    card_id: &'a str,
    stage: &'a str,
    strategy_json: &'a str,
    worktree_path: Option<&'a str>,
) -> UpsertStageRunInput<'a> {
    UpsertStageRunInput {
        id,
        card_id,
        stage,
        task_id: None,
        attempt: 1,
        stable_launch_key: "card-dur:plan:key-1",
        card_revision: 2,
        provider_instance_id: "devReplay",
        model: "dev-replay",
        model_options_json: None,
        runtime_mode: "auto-accept-edits",
        interaction_mode: if stage == "plan" { "plan" } else { "default" },
        workspace_strategy_json: strategy_json,
        worktree_path,
        branch: worktree_path.map(|_| "feat/dur"),
        brief_digest: Some("sha256:brief"),
    }
}

#[test]
fn stage_run_launch_key_idempotency_and_reopen_durability() -> TestResult {
    let dir = tempfile::tempdir()?;
    let db_path = dir.path().join("stage_runs.sqlite");

    {
        let store = SqliteStore::open(&db_path)?;
        let conn = store.connection();
        seed_approved_card(conn, "card-dur")?;

        let UpsertStageRunOutcome::Created(sr) = upsert_or_get_stage_run_by_launch_key(
            conn,
            &sample_stage_run_input(
                "sr-1",
                "card-dur",
                "plan",
                "{\"type\":\"existing_worktree\",\"worktreePath\":\"/wt/1\"}",
                Some("/wt/1"),
            ),
            "2026-11-24T00:02:00Z",
        )?
        else {
            return Err("expected Created".into());
        };

        link_stage_run_thread(
            conn,
            &LinkStageRunThreadInput {
                id: &sr.id,
                status: "running",
                thread_id: Some("thr-dur-1"),
                workflow_run_id: Some("wf-dur-1"),
                worktree_path: Some("/wt/1"),
                branch: Some("feat/dur"),
                last_error: None,
                ended_at: None,
            },
            "2026-11-24T00:03:00Z",
        )?;
        store.close()?;
    }

    let reopened = SqliteStore::open(&db_path)?;
    let conn = reopened.connection();
    let detail = get_card_detail(conn, "card-dur")?;
    assert_eq!(detail.card.revision, 2);
    assert_eq!(detail.card.stage, "plan");
    assert_eq!(detail.approvals[0].status, "valid");

    let outcome = upsert_or_get_stage_run_by_launch_key(
        conn,
        &sample_stage_run_input("sr-dup", "card-dur", "plan", "{\"type\":\"root\"}", None),
        "2026-11-24T00:05:00Z",
    )?;
    assert!(matches!(
        outcome,
        UpsertStageRunOutcome::Existing(ref r)
            if r.id == "sr-1" && r.thread_id.as_deref() == Some("thr-dur-1")
    ));
    assert_eq!(list_stage_runs_for_card(conn, "card-dur")?.len(), 1);

    assert!(matches!(
        upsert_or_get_stage_run_by_launch_key(
            conn,
            &sample_stage_run_input(
                "sr-conflict",
                "card-dur",
                "execute",
                "{\"type\":\"root\"}",
                None
            ),
            "2026-11-24T00:05:30Z",
        ),
        Err(WorkflowBoardRepoError::ReuseConflict(_))
    ));
    Ok(())
}

#[test]
fn write_journal_transitions_and_crash_recovery_across_reopen() -> TestResult {
    let dir = tempfile::tempdir()?;
    let db_path = dir.path().join("journal.sqlite");

    {
        let store = SqliteStore::open(&db_path)?;
        let conn = store.connection();
        seed_approved_card(conn, "card-j")?;

        stage_write(
            conn,
            &StageWriteInput {
                id: "j-1",
                card_id: "card-j",
                target_path: "spec.md",
                staging_path: Some("spec.md.tmp"),
                operation: "write",
                expected_digest: Some("sha256:initial"),
                previous_digest: None,
                revision: Some(2),
            },
            "2026-11-24T00:04:00Z",
        )?;
        mark_file_written(conn, "j-1", "2026-11-24T00:04:05Z")?;

        stage_write(
            conn,
            &StageWriteInput {
                id: "j-2",
                card_id: "card-j",
                target_path: "plan-brief.md",
                staging_path: Some("plan-brief.md.tmp"),
                operation: "write",
                expected_digest: Some("sha256:brief"),
                previous_digest: None,
                revision: Some(2),
            },
            "2026-11-24T00:04:10Z",
        )?;
        store.close()?;
    }

    let reopened = SqliteStore::open(&db_path)?;
    let conn = reopened.connection();
    assert_eq!(get_card_detail(conn, "card-j")?.incomplete_journal_count, 2);

    let incomplete = list_incomplete_journal_entries(conn, None)?;
    assert_eq!(incomplete.len(), 2);
    assert_eq!(incomplete[0].status, "file_written");
    assert_eq!(incomplete[1].status, "staged");

    let committed = commit_journal(conn, "j-1", "2026-11-24T00:06:00Z")?;
    assert_eq!(committed.status, "committed");
    let rolled = rollback_journal(
        conn,
        "j-2",
        Some("recovered on restart"),
        "2026-11-24T00:06:01Z",
    )?;
    assert_eq!(rolled.status, "rolled_back");
    assert_eq!(rolled.last_error.as_deref(), Some("recovered on restart"));

    assert_eq!(
        list_incomplete_journal_entries(conn, Some("card-j"))?.len(),
        0
    );
    assert_eq!(get_card_detail(conn, "card-j")?.incomplete_journal_count, 0);
    Ok(())
}
