//! Integration tests for `workflow_board_repo`: CRUD, CAS concurrency, and approval binding/invalidation.

use legion_persistence::sqlite::SqliteStore;
use legion_persistence::workflow_board_repo::{
    ApproveSpecInput, CreateCardInput, UpdateSpecInput, WorkflowBoardRepoError, approve_spec,
    create_card, get_board_snapshot, get_card_detail, list_project_cards, update_spec,
};
use rusqlite::params;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn sample_create<'a>(card_id: &'a str, project_id: &'a str, title: &'a str) -> CreateCardInput<'a> {
    CreateCardInput {
        card_id,
        revision_id: Box::leak(format!("rev-{card_id}-1").into_boxed_str()),
        project_id,
        title,
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
        spec_json: "{\"constraints\":[\"No auto-commits\"]}",
        spec_digest: Some("sha256:initial"),
        wireframes_digest: None,
    }
}

#[test]
fn crud_snapshot_ordering_and_not_found_errors() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();

    assert!(matches!(
        get_card_detail(conn, "missing-card"),
        Err(WorkflowBoardRepoError::CardNotFound { .. })
    ));

    create_card(
        conn,
        &sample_create("card-a", "proj-1", "First Card"),
        "2026-11-24T00:00:00Z",
    )?;
    create_card(
        conn,
        &sample_create("card-b", "proj-1", "Second Card"),
        "2026-11-24T01:00:00Z",
    )?;
    create_card(
        conn,
        &sample_create("card-other", "proj-2", "Other Project Card"),
        "2026-11-24T02:00:00Z",
    )?;

    let proj1_cards = list_project_cards(conn, "proj-1")?;
    assert_eq!(proj1_cards.len(), 2);
    assert_eq!(proj1_cards[0].card_id, "card-b");
    assert_eq!(proj1_cards[1].card_id, "card-a");

    let snap = get_board_snapshot(conn, "proj-1", "2026-11-24T03:00:00Z")?;
    assert_eq!(snap.project_id, "proj-1");
    assert_eq!(snap.revision, 1);
    assert_eq!(snap.cards.len(), 2);

    let detail = get_card_detail(conn, "card-a")?;
    assert_eq!(detail.revisions.len(), 1);
    assert_eq!(detail.revisions[0].kind, "card_created");
    Ok(())
}

#[test]
fn cas_revision_conflicts_and_multi_connection_contention() -> TestResult {
    let dir = tempfile::tempdir()?;
    let db_path = dir.path().join("cas.sqlite");

    let store1 = SqliteStore::open(&db_path)?;
    let store2 = SqliteStore::open(&db_path)?;

    let created = create_card(
        store1.connection(),
        &sample_create("card-cas", "proj-1", "CAS Card"),
        "2026-11-24T00:00:00Z",
    )?;
    assert_eq!(created.card.revision, 1);

    let updated = update_spec(
        store1.connection(),
        &UpdateSpecInput {
            card_id: "card-cas",
            revision_id: "rev-2",
            expected_revision: 1,
            title: None,
            brief: Some("Winner update"),
            intake_goal: None,
            input_paths_json: None,
            spec_json: Some("{\"updated\":true}"),
            spec_digest: Some("sha256:winner"),
            wireframes_digest: None,
            clear_wireframes_digest: false,
        },
        "2026-11-24T00:01:00Z",
    )?;
    assert_eq!(updated.card.revision, 2);

    // Competing second connection using stale expected_revision=1 must get RevisionConflict.
    let conflict = update_spec(
        store2.connection(),
        &UpdateSpecInput {
            card_id: "card-cas",
            revision_id: "rev-2-stale",
            expected_revision: 1,
            title: None,
            brief: Some("Loser update"),
            intake_goal: None,
            input_paths_json: None,
            spec_json: None,
            spec_digest: Some("sha256:loser"),
            wireframes_digest: None,
            clear_wireframes_digest: false,
        },
        "2026-11-24T00:02:00Z",
    );
    assert!(matches!(
        conflict,
        Err(WorkflowBoardRepoError::RevisionConflict {
            expected_revision: 1,
            actual_revision: 2,
            ..
        })
    ));

    // Also verify approve_spec enforces expected_revision strictly.
    let approve_conflict = approve_spec(
        store2.connection(),
        &ApproveSpecInput {
            card_id: "card-cas",
            revision_id: "rev-3-stale",
            approval_id: "apr-stale",
            expected_revision: 1,
            expected_spec_digest: None,
            wireframes_digest: None,
            approved_by: "client",
        },
        "2026-11-24T00:03:00Z",
    );
    assert!(matches!(
        approve_conflict,
        Err(WorkflowBoardRepoError::RevisionConflict {
            expected_revision: 1,
            actual_revision: 2,
            ..
        })
    ));
    Ok(())
}

#[test]
fn spec_approval_requires_digest_and_binds_to_revision() -> TestResult {
    let dir = tempfile::tempdir()?;
    let db_path = dir.path().join("approvals-bind.sqlite");
    let store = SqliteStore::open(&db_path)?;
    let conn = store.connection();

    create_card(
        conn,
        &CreateCardInput {
            spec_digest: None,
            ..sample_create("card-apr", "proj-1", "Approval Card")
        },
        "2026-11-24T00:00:00Z",
    )?;

    assert!(matches!(
        approve_spec(
            conn,
            &ApproveSpecInput {
                card_id: "card-apr",
                revision_id: "rev-fail",
                approval_id: "apr-fail",
                expected_revision: 1,
                expected_spec_digest: None,
                wireframes_digest: None,
                approved_by: "client",
            },
            "2026-11-24T00:01:00Z",
        ),
        Err(WorkflowBoardRepoError::PreconditionFailed(_))
    ));

    let rev2 = update_spec(
        conn,
        &UpdateSpecInput {
            card_id: "card-apr",
            revision_id: "rev-2",
            expected_revision: 1,
            title: None,
            brief: Some("Spec v1"),
            intake_goal: None,
            input_paths_json: None,
            spec_json: Some("{\"ac\":[\"AC-01\"]}"),
            spec_digest: Some("sha256:spec-v1"),
            wireframes_digest: None,
            clear_wireframes_digest: false,
        },
        "2026-11-24T00:02:00Z",
    )?;
    assert_eq!(rev2.card.revision, 2);

    let rev3 = approve_spec(
        conn,
        &ApproveSpecInput {
            card_id: "card-apr",
            revision_id: "rev-3",
            approval_id: "apr-1",
            expected_revision: 2,
            expected_spec_digest: Some("sha256:spec-v1"),
            wireframes_digest: None,
            approved_by: "client",
        },
        "2026-11-24T00:03:00Z",
    )?;
    assert_eq!(rev3.card.revision, 3);
    assert_eq!(rev3.card.stage, "plan");
    assert_eq!(rev3.approvals[0].status, "valid");
    assert_eq!(rev3.approvals[0].revision, 3);
    assert_eq!(
        rev3.approvals[0].spec_digest.as_deref(),
        Some("sha256:spec-v1")
    );
    Ok(())
}

#[test]
fn spec_update_stales_approvals_on_change_and_preserves_when_unchanged() -> TestResult {
    let dir = tempfile::tempdir()?;
    let db_path = dir.path().join("approvals-stale.sqlite");
    let store = SqliteStore::open(&db_path)?;
    let conn = store.connection();

    create_card(
        conn,
        &sample_create("card-apr", "proj-1", "Approval Card"),
        "2026-11-24T00:00:00Z",
    )?;
    let rev2 = approve_spec(
        conn,
        &ApproveSpecInput {
            card_id: "card-apr",
            revision_id: "rev-2",
            approval_id: "apr-1",
            expected_revision: 1,
            expected_spec_digest: Some("sha256:initial"),
            wireframes_digest: None,
            approved_by: "client",
        },
        "2026-11-24T00:01:00Z",
    )?;
    assert_eq!(rev2.card.revision, 2);

    // Simulate an active 'valid' plan approval as well to verify spec edit stales BOTH spec and plan.
    conn.execute(
        "INSERT INTO workflow_board_approvals (id, card_id, gate, status, revision, artifact_digest, spec_digest, plan_digest, approved_at)
         VALUES ('apr-plan-1', 'card-apr', 'plan', 'valid', 2, 'sha256:plan-v1', 'sha256:initial', 'sha256:plan-v1', '2026-11-24T00:02:00Z')",
        params![],
    )?;

    // Idempotent update_spec with unchanged content/digest preserves valid approvals and revision.
    let unchanged = update_spec(
        conn,
        &UpdateSpecInput {
            card_id: "card-apr",
            revision_id: "rev-noop",
            expected_revision: 2,
            title: None,
            brief: Some("Initial brief"),
            intake_goal: None,
            input_paths_json: None,
            spec_json: Some("{\"constraints\":[\"No auto-commits\"]}"),
            spec_digest: Some("sha256:initial"),
            wireframes_digest: None,
            clear_wireframes_digest: false,
        },
        "2026-11-24T00:03:00Z",
    )?;
    assert_eq!(unchanged.card.revision, 2);
    assert!(unchanged.approvals.iter().all(|a| a.status == "valid"));

    // Modifying spec content/digest marks both active 'spec' and 'plan' approvals 'stale'.
    let rev3 = update_spec(
        conn,
        &UpdateSpecInput {
            card_id: "card-apr",
            revision_id: "rev-3",
            expected_revision: 2,
            title: None,
            brief: Some("Spec v2"),
            intake_goal: None,
            input_paths_json: None,
            spec_json: Some("{\"ac\":[\"AC-01\",\"AC-02\"]}"),
            spec_digest: Some("sha256:spec-v2"),
            wireframes_digest: None,
            clear_wireframes_digest: false,
        },
        "2026-11-24T00:04:00Z",
    )?;
    assert_eq!(rev3.card.revision, 3);
    assert!(rev3.approvals.iter().all(|a| a.status == "stale"));

    // Re-approving creates a new 'valid' spec approval bound to rev 4 + sha256:spec-v2.
    let rev4 = approve_spec(
        conn,
        &ApproveSpecInput {
            card_id: "card-apr",
            revision_id: "rev-4",
            approval_id: "apr-2",
            expected_revision: 3,
            expected_spec_digest: Some("sha256:spec-v2"),
            wireframes_digest: None,
            approved_by: "client",
        },
        "2026-11-24T00:05:00Z",
    )?;
    assert_eq!(rev4.card.revision, 4);
    assert_eq!(rev4.approvals[0].id, "apr-2");
    assert_eq!(rev4.approvals[0].status, "valid");
    assert_eq!(
        rev4.approvals[0].spec_digest.as_deref(),
        Some("sha256:spec-v2")
    );
    Ok(())
}

#[test]
fn create_card_validates_persists_and_hydrates_brief_path() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();

    let with_brief = create_card(
        conn,
        &CreateCardInput {
            brief_path: Some("  docs/brief.md  "),
            ..sample_create("card-bp-some", "proj-1", "Brief Path Card")
        },
        "2026-11-24T00:00:00Z",
    )?;
    assert_eq!(with_brief.card.brief_path.as_deref(), Some("docs/brief.md"));
    let reloaded = get_card_detail(conn, "card-bp-some")?;
    assert_eq!(reloaded.card.brief_path.as_deref(), Some("docs/brief.md"));
    let db_brief_path: Option<String> = conn.query_row(
        "SELECT brief_path FROM workflow_board_cards WHERE card_id = 'card-bp-some'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(db_brief_path.as_deref(), Some("docs/brief.md"));

    let without_brief = create_card(
        conn,
        &sample_create("card-bp-none", "proj-1", "No Brief Path Card"),
        "2026-11-24T00:01:00Z",
    )?;
    assert_eq!(without_brief.card.brief_path, None);
    let db_none: Option<String> = conn.query_row(
        "SELECT brief_path FROM workflow_board_cards WHERE card_id = 'card-bp-none'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(db_none, None);

    for bad in [".env", ".ssh/config", "keys/id_rsa", "../x", "", "   "] {
        let err = create_card(
            conn,
            &CreateCardInput {
                brief_path: Some(bad),
                ..sample_create("card-bp-bad", "proj-1", "Bad Brief Path Card")
            },
            "2026-11-24T00:02:00Z",
        );
        assert!(
            matches!(err, Err(WorkflowBoardRepoError::InvalidInput(_))),
            "expected InvalidInput for brief_path={bad:?}"
        );
    }
    let bad_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM workflow_board_cards WHERE card_id = 'card-bp-bad'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(bad_count, 0);
    Ok(())
}
