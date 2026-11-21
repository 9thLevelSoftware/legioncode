//! Integration and property tests for `t3_scm::checkpoints`.

use proptest::prelude::*;
use rusqlite::params;
use t3_contracts::ids::{CheckpointId, CheckpointScopeId, CommandId, ThreadId};
use t3_persistence::sqlite::SqliteStore;
use t3_scm::checkpoints::diff_query::{
    CheckpointDiffQuery, DIFF_TRUNCATED_MARKER, DiffFileInput, DiffQueryOptions,
    MAX_PATHSPEC_CHARS, batch_pathspecs, parse_numstat_z, truncate_diff_utf8,
};
use t3_scm::checkpoints::rollback::{RollbackCoordinator, RollbackIntent};
use t3_scm::checkpoints::store::{CheckpointDiffBlob, CheckpointStore};
use t3_scm::checkpoints::{CheckpointError, checkpoint_ref_for_turn, parse_checkpoint_ref};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn checkpoint_ref_roundtrip_and_git_ref_safety() -> TestResult {
    let tid = ThreadId::new("thread-alpha_42")?;
    let cp_ref = checkpoint_ref_for_turn(&tid, 7)?;
    assert_eq!(
        cp_ref.as_str(),
        "refs/t3/checkpoints/thread-alpha_42/turn-7"
    );
    assert_eq!(
        parse_checkpoint_ref(cp_ref.as_str()),
        Some(("thread-alpha_42".to_owned(), 7))
    );

    for unsafe_id in [
        "thread:1",
        "thread..2",
        "bad/slash",
        "bad.lock",
        "bad@{ref",
        "has space",
        "-leading-dash",
    ] {
        let parsed = ThreadId::new(unsafe_id)?;
        let res = checkpoint_ref_for_turn(&parsed, 1);
        assert!(
            matches!(res, Err(CheckpointError::InvalidRef { .. })),
            "expected InvalidRef for `{unsafe_id}`"
        );
    }
    Ok(())
}

#[test]
fn checkpoint_store_crud_and_prune_after_turn() -> TestResult {
    let db = SqliteStore::open_in_memory()?;
    let store = CheckpointStore::new(db.connection());
    let tid = ThreadId::new("thread-store-1")?;

    store.save_diff_blob(&CheckpointDiffBlob {
        thread_id: tid.clone(),
        from_turn_count: 0,
        to_turn_count: 1,
        diff: "diff-0-1".to_owned(),
        created_at: "2026-11-20T01:00:00Z".to_owned(),
    })?;
    store.save_diff_blob(&CheckpointDiffBlob {
        thread_id: tid.clone(),
        from_turn_count: 1,
        to_turn_count: 2,
        diff: "diff-1-2".to_owned(),
        created_at: "2026-11-20T02:00:00Z".to_owned(),
    })?;
    store.save_diff_blob(&CheckpointDiffBlob {
        thread_id: tid.clone(),
        from_turn_count: 2,
        to_turn_count: 3,
        diff: "diff-2-3-v1".to_owned(),
        created_at: "2026-11-20T03:00:00Z".to_owned(),
    })?;
    // Upsert existing key replaces diff and timestamp.
    store.save_diff_blob(&CheckpointDiffBlob {
        thread_id: tid.clone(),
        from_turn_count: 2,
        to_turn_count: 3,
        diff: "diff-2-3-v2".to_owned(),
        created_at: "2026-11-20T03:05:00Z".to_owned(),
    })?;

    let invalid_range = store.save_diff_blob(&CheckpointDiffBlob {
        thread_id: tid.clone(),
        from_turn_count: 4,
        to_turn_count: 2,
        diff: String::new(),
        created_at: "2026-11-20T04:00:00Z".to_owned(),
    });
    assert!(matches!(
        invalid_range,
        Err(CheckpointError::InvalidInput(_))
    ));

    let got = store.get_diff_blob(&tid, 2, 3)?;
    assert_eq!(got.as_ref().map(|b| b.diff.as_str()), Some("diff-2-3-v2"));

    let list = store.list_thread_checkpoints(&tid)?;
    assert_eq!(list.len(), 3);
    assert_eq!(list[0].to_turn_count, 1);
    assert_eq!(list[2].to_turn_count, 3);
    assert_eq!(
        list[2].checkpoint_ref.as_deref(),
        Some("refs/t3/checkpoints/thread-store-1/turn-3")
    );

    let deleted = store.delete_blobs_after_turn(&tid, 1)?;
    assert_eq!(deleted, 2);
    assert!(store.get_diff_blob(&tid, 0, 1)?.is_some());
    assert!(store.get_diff_blob(&tid, 1, 2)?.is_none());
    Ok(())
}

#[test]
fn diff_query_preserves_filename_whitespace_and_ignores_content_whitespace() {
    let weird_path = "  spaced dir / file  name .rs ";
    let ws_only = DiffFileInput {
        old_path: weird_path,
        new_path: weird_path,
        old_content: "fn main() {\n    println!(\"hi\");\n}\n",
        new_content: "fn   main() {\n\tprintln!(\"hi\");\n}\n",
        is_binary: false,
    };
    let suppressed = CheckpointDiffQuery::compute_files(
        &[ws_only],
        DiffQueryOptions {
            ignore_whitespace: true,
            ..DiffQueryOptions::default()
        },
    );
    assert_eq!(suppressed.diff, "");
    assert_eq!(suppressed.files_changed, 0);

    let real_edit = DiffFileInput {
        old_path: weird_path,
        new_path: weird_path,
        old_content: "    let a = 1;\n    let b = 2;\n",
        new_content: "  let a = 1;\n    let b = 99;\n",
        is_binary: false,
    };
    let rendered = CheckpointDiffQuery::compute_files(
        &[real_edit],
        DiffQueryOptions {
            ignore_whitespace: true,
            context_lines: 3,
            ..DiffQueryOptions::default()
        },
    );
    assert!(rendered.diff.contains(&format!("--- a/{weird_path}\n")));
    assert!(rendered.diff.contains(&format!("+++ b/{weird_path}\n")));
    // First line only changed indentation so remains unchanged context in original form;
    // second line preserves actual indentation and changed value.
    assert!(rendered.diff.contains("     let a = 1;\n"));
    assert!(rendered.diff.contains("-    let b = 2;\n"));
    assert!(rendered.diff.contains("+    let b = 99;\n"));
    assert_eq!(rendered.files_changed, 1);
    assert_eq!(rendered.insertions, 1);
    assert_eq!(rendered.deletions, 1);
}

#[test]
fn diff_utf8_truncation_numstat_z_and_pathspec_batching() {
    let sample = "abc🚀def🔥ghijklmno";
    let (cut, truncated) = truncate_diff_utf8(sample, 4 + DIFF_TRUNCATED_MARKER.len());
    assert!(truncated);
    assert!(cut.ends_with(DIFF_TRUNCATED_MARKER));
    assert!(cut.len() <= 4 + DIFF_TRUNCATED_MARKER.len());

    let raw_numstat =
        b"12\t3\t spaced file.rs\0-\t-\tassets/logo.png\x004\t1\t\0old dir/a.txt\0new dir/a .txt\0";
    let parsed = parse_numstat_z(raw_numstat);
    assert_eq!(parsed.len(), 3);
    assert_eq!(parsed[0].path, " spaced file.rs");
    assert_eq!(parsed[0].insertions, Some(12));
    assert_eq!(parsed[1].insertions, None);
    assert_eq!(parsed[2].old_path.as_deref(), Some("old dir/a.txt"));
    assert_eq!(parsed[2].path, "new dir/a .txt");

    let long_a = "a".repeat(15_000);
    let long_b = "b".repeat(15_000);
    let batches = batch_pathspecs(&[&long_a, &long_b], MAX_PATHSPEC_CHARS);
    assert_eq!(batches.len(), 2);
}

#[test]
fn rollback_coordinator_bug02_enforces_durable_pending_intent() -> TestResult {
    let db = SqliteStore::open_in_memory()?;
    let conn = db.connection();

    let thread_id = ThreadId::new("thread-rb-1")?;
    let command_id = CommandId::new("cmd-rb-1")?;
    let checkpoint_id = CheckpointId::new("cp-1")?;
    let scope_id = CheckpointScopeId::new("scope-main")?;
    let checkpoint_ref = checkpoint_ref_for_turn(&thread_id, 2)?;

    let intent = RollbackIntent {
        thread_id: thread_id.clone(),
        command_id: command_id.clone(),
        checkpoint_id: checkpoint_id.clone(),
        scope_id: Some(scope_id.clone()),
        checkpoint_ref,
        restore_files: true,
        worktree_is_shared: false,
    };

    // 1. Fails when database has no pending intent.
    assert!(matches!(
        RollbackCoordinator::plan_git_restore_checked(conn, &intent, &["src/lib.rs"], false),
        Err(CheckpointError::RollbackGuard(_))
    ));

    // Seed projection/event/outbox incrementally and assert failure until ALL durable invariants hold.
    let outbox_payload = serde_json::json!({
        "type": "provider-thread.rollback",
        "intent": "rollback.pending",
        "commandId": command_id.as_str(),
        "checkpointId": checkpoint_id.as_str(),
        "scopeId": scope_id.as_str(),
        "restoreFiles": true,
    })
    .to_string();

    conn.execute(
        "INSERT INTO orchestration_v2_effect_outbox
         (effect_id, command_id, thread_id, effect_type, payload_json, status, attempt_count, available_at, created_at, updated_at)
         VALUES (?1, ?2, ?3, 'provider-thread.rollback', ?4, 'succeeded', 1, '2026-11-20T00:00:00Z', '2026-11-20T00:00:00Z', '2026-11-20T00:00:00Z')",
        params!["eff-1", command_id.as_str(), thread_id.as_str(), outbox_payload],
    )?;
    // Status is `succeeded`, NOT pending/running -> still blocked!
    assert!(matches!(
        RollbackCoordinator::verify_pending_intent(conn, &intent),
        Err(CheckpointError::RollbackGuard(_))
    ));

    conn.execute(
        "UPDATE orchestration_v2_effect_outbox SET status = 'pending' WHERE effect_id = 'eff-1'",
        [],
    )?;
    // Still missing projection & orchestration event -> blocked!
    assert!(matches!(
        RollbackCoordinator::verify_pending_intent(conn, &intent),
        Err(CheckpointError::RollbackGuard(_))
    ));

    let proj_payload = serde_json::json!({
        "threadId": thread_id.as_str(),
        "rollbackRequestId": command_id.as_str(),
    })
    .to_string();
    conn.execute(
        "INSERT INTO orchestration_v2_projection_threads
         (thread_id, project_id, title, default_provider, runtime_mode, interaction_mode, active_provider_thread_id, created_at, updated_at, archived_at, deleted_at, payload_json, provider_instance_id)
         VALUES (?1, 'proj-1', 'Rollback Thread', 'codex', 'full-access', 'default', NULL, '2026-11-20T00:00:00Z', '2026-11-20T00:00:00Z', NULL, NULL, ?2, NULL)",
        params![thread_id.as_str(), proj_payload],
    )?;
    // Still missing event -> blocked!
    assert!(matches!(
        RollbackCoordinator::verify_pending_intent(conn, &intent),
        Err(CheckpointError::RollbackGuard(_))
    ));

    conn.execute(
        "INSERT INTO orchestration_events
         (event_id, aggregate_kind, stream_id, stream_version, event_type, occurred_at, command_id, causation_event_id, correlation_id, actor_kind, payload_json, metadata_json, application_event_version)
         VALUES ('evt-1', 'thread', ?1, 5, 'checkpoint.rollback-requested', '2026-11-20T00:00:00Z', ?2, NULL, NULL, 'server', '{}', '{}', 1)",
        params![thread_id.as_str(), command_id.as_str()],
    )?;

    // Shared worktree file restore must STILL be refused even with durable DB state.
    let shared_intent = RollbackIntent {
        worktree_is_shared: true,
        ..intent.clone()
    };
    assert!(matches!(
        RollbackCoordinator::verify_pending_intent(conn, &shared_intent),
        Err(CheckpointError::RollbackGuard(_))
    ));

    // All BUG-02 invariants verified -> git restore args can now be emitted!
    assert_git_restore_plan(conn, &intent)?;
    Ok(())
}

fn assert_git_restore_plan(conn: &rusqlite::Connection, intent: &RollbackIntent) -> TestResult {
    let plan = RollbackCoordinator::plan_git_restore_checked(
        conn,
        intent,
        &["src/lib.rs", " spaced file.rs"],
        true,
    )?;
    assert_eq!(plan.checkout_ref, "refs/t3/checkpoints/thread-rb-1/turn-2");
    assert_eq!(
        plan.restore_batches,
        vec![vec![
            "restore".to_owned(),
            "--source".to_owned(),
            "refs/t3/checkpoints/thread-rb-1/turn-2".to_owned(),
            "--worktree".to_owned(),
            "--staged".to_owned(),
            "--".to_owned(),
            "src/lib.rs".to_owned(),
            " spaced file.rs".to_owned(),
        ]]
    );
    assert_eq!(
        plan.clean_untracked_args,
        Some(vec![
            "clean".to_owned(),
            "-fd".to_owned(),
            "--".to_owned(),
            ".".to_owned(),
        ])
    );
    Ok(())
}

proptest! {
    #[test]
    fn prop_checkpoint_ref_roundtrips_safe_thread_ids(
        raw_id in "[a-zA-Z0-9][a-zA-Z0-9_-]{0,20}",
        turn in 0u32..10_000u32,
    ) {
        let tid = ThreadId::new(raw_id.clone()).map_err(|e| TestCaseError::fail(e.to_string()))?;
        let cp_ref = checkpoint_ref_for_turn(&tid, turn).map_err(|e| TestCaseError::fail(e.to_string()))?;
        let parsed = parse_checkpoint_ref(cp_ref.as_str());
        prop_assert_eq!(parsed, Some((raw_id, turn)));
    }

    #[test]
    fn prop_truncate_diff_always_valid_utf8_and_within_limit(
        input in ".*",
        max_bytes in 0usize..128usize,
    ) {
        let (out, truncated) = truncate_diff_utf8(&input, max_bytes);
        prop_assert!(out.len() <= max_bytes);
        prop_assert!(std::str::from_utf8(out.as_bytes()).is_ok());
        prop_assert_eq!(truncated, input.len() > max_bytes);
    }
}
