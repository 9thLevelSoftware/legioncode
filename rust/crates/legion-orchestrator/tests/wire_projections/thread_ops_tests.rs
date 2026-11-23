use crate::{
    TestResult, insert_msg_row, insert_project, insert_run_row, insert_thread_row,
    insert_turn_item_row,
};
use legion_contracts::ids::{CommandId, ProjectId, ThreadId};
use legion_contracts::orchestration::{DomainEvent, OrchestrationCommand};
use legion_orchestrator::sink::CommitCommandInput;
use legion_orchestrator::{
    Orchestrator, ProjectionError, query_wire_thread_bounded, query_wire_thread_detail,
    query_wire_thread_history_page, query_wire_thread_transcript, render_wire_thread_transcript,
};
use legion_persistence::sqlite::SqliteStore;
use rusqlite::Connection;
use serde_json::{Value, json};

fn seed_fork_fixture(conn: &Connection) -> TestResult {
    insert_project(conn, "p-1", "Alpha", None)?;
    insert_thread_row(
        conn,
        "t-parent",
        "p-1",
        "Parent",
        "2026-11-20T00:00:00.000Z",
        &json!({}),
    )?;
    insert_thread_row(
        conn,
        "t-child",
        "p-1",
        "Forked",
        "2026-11-20T00:05:00.000Z",
        &json!({ "forkedFrom": { "type": "run", "threadId": "t-parent", "runId": "r-p1", "capturedAt": "2026-11-20T00:01:00.000Z" } }),
    )?;
    insert_run_row(
        conn,
        "r-p1",
        "t-parent",
        1,
        "completed",
        "2026-11-20T00:00:10Z",
    )?;
    insert_run_row(
        conn,
        "r-c1",
        "t-child",
        1,
        "completed",
        "2026-11-20T00:02:00Z",
    )?;
    insert_run_row(
        conn,
        "r-rb",
        "t-child",
        2,
        "rolled_back",
        "2026-11-20T00:03:00Z",
    )?;
    insert_msg_row(
        conn,
        "m-p1",
        "t-parent",
        "r-p1",
        "user",
        "2026-11-20T00:00:11Z",
        "Inherited ask",
    )?;
    insert_msg_row(
        conn,
        "m-c1",
        "t-child",
        "r-c1",
        "user",
        "2026-11-20T00:02:01Z",
        "Followup",
    )?;
    insert_msg_row(
        conn,
        "m-c2",
        "t-child",
        "r-c1",
        "assistant",
        "2026-11-20T00:02:02Z",
        "Answer",
    )?;
    insert_msg_row(
        conn,
        "m-rb",
        "t-child",
        "r-rb",
        "assistant",
        "2026-11-20T00:03:01Z",
        "Rolled back",
    )?;
    let big_out = "x".repeat(600);
    for item in [
        json!({ "id": "ti-p1", "threadId": "t-parent", "runId": "r-p1", "type": "user_message", "messageId": "m-p1", "text": "Inherited ask", "ordinal": 1, "status": "completed", "updatedAt": "2026-11-20T00:00:11Z" }),
        json!({ "id": "ti-c1", "threadId": "t-child", "runId": "r-c1", "type": "user_message", "messageId": "m-c1", "text": "Followup", "ordinal": 1, "status": "completed", "updatedAt": "2026-11-20T00:02:01Z" }),
        json!({ "id": "ti-c2", "threadId": "t-child", "runId": "r-c1", "type": "command_execution", "command": "cargo test", "status": "failed", "exitCode": 1, "output": big_out, "ordinal": 2, "updatedAt": "2026-11-20T00:02:02Z" }),
        json!({ "id": "ti-c3", "threadId": "t-child", "runId": "r-c1", "type": "assistant_message", "messageId": "m-c2", "text": "Answer", "ordinal": 3, "status": "completed", "updatedAt": "2026-11-20T00:02:03Z" }),
        json!({ "id": "ti-rb", "threadId": "t-child", "runId": "r-rb", "type": "assistant_message", "messageId": "m-rb", "text": "Rolled back", "ordinal": 4, "status": "completed", "updatedAt": "2026-11-20T00:03:01Z" }),
    ] {
        insert_turn_item_row(conn, &item)?;
    }
    Ok(())
}

#[test]
fn wire_thread_detail_bounded_paging_transcript_and_capping() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    seed_fork_fixture(conn)?;

    let detail = query_wire_thread_detail(conn, "t-child")?;
    let visible_ids: Vec<&str> = detail
        .projection
        .visible_turn_items
        .iter()
        .map(|v| v.source_item_id.as_str())
        .collect();
    assert_eq!(
        visible_ids,
        vec!["ti-p1", "turn-item:fork:t-child", "ti-c1", "ti-c2", "ti-c3"]
    );
    let cmd_item = &detail.projection.visible_turn_items[3].item;
    assert!(cmd_item.get("output").is_none());
    assert_eq!(cmd_item["outputOmitted"], true);
    assert_eq!(cmd_item["outputIndicatesFailure"], true);

    let bounded = query_wire_thread_bounded(conn, "t-child", Some(2))?;
    assert_eq!(bounded.projection.visible_turn_items.len(), 2);
    assert!(bounded.has_more_history);
    let cursor = bounded.history_cursor.as_deref().ok_or("historyCursor")?;
    let page = query_wire_thread_history_page(conn, "t-child", Some(cursor), Some(5))?;
    assert_eq!(page.items.len(), 3);
    assert!(!page.has_more_history);

    assert!(matches!(
        query_wire_thread_history_page(conn, "t-child", Some("bad"), Some(5)),
        Err(ProjectionError::InvalidCursor)
    ));
    assert!(matches!(
        query_wire_thread_detail(conn, "missing"),
        Err(ProjectionError::ThreadNotFound(_))
    ));

    let transcript = query_wire_thread_transcript(conn, "t-child")?;
    assert_eq!(transcript.title, "Forked");
    let md = render_wire_thread_transcript(conn, "t-child")?;
    assert!(md.contains("Inherited ask"));
    assert!(md.contains("Answer"));
    Ok(())
}

#[tokio::test]
async fn decider_client_ops_and_extended_event_sink_materialize() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let orch = Orchestrator::from_sqlite_store(store);
    let _ = ProjectId::new("p-ops")?;
    for cmd_json in [
        json!({ "type": "thread.create", "createdBy": "user", "creationSource": "web", "commandId": "c-1", "threadId": "t-ops", "projectId": "p-ops", "title": "Initial", "modelSelection": { "instanceId": "codex", "model": "gpt-5" }, "runtimeMode": "full-access", "interactionMode": "default", "branch": null, "worktreePath": null }),
        json!({ "type": "thread.metadata.update", "commandId": "c-2", "threadId": "t-ops", "title": "Renamed", "branch": "feat/wire" }),
        json!({ "type": "thread.runtime-mode.set", "commandId": "c-3", "threadId": "t-ops", "runtimeMode": "approval-required" }),
        json!({ "type": "thread.tag.set", "commandId": "c-4", "threadId": "t-ops", "tag": "wire", "tagged": true }),
        json!({ "type": "thread.tag.set", "commandId": "c-4b", "threadId": "t-ops", "tag": "p2a", "tagged": true }),
        json!({ "type": "thread.auto-settle.set", "commandId": "c-5", "threadId": "t-ops", "enabled": false }),
        json!({ "type": "message.dispatch", "createdBy": "user", "creationSource": "web", "commandId": "c-m1", "threadId": "t-ops", "messageId": "m-1", "text": "Run 1", "attachments": [], "dispatchMode": { "type": "start_immediately" } }),
        json!({ "type": "message.dispatch", "createdBy": "user", "creationSource": "web", "commandId": "c-m2", "threadId": "t-ops", "messageId": "m-2", "text": "Queued 2", "attachments": [], "dispatchMode": { "type": "queue_after_active" } }),
    ] {
        let cmd: OrchestrationCommand = serde_json::from_value(cmd_json)?;
        orch.dispatch_command(&cmd, "2026-11-20T00:01:00.000Z")
            .await?;
    }

    let runs = {
        let conn = orch.sink().db().lock().map_err(|_| "lock poisoned")?;
        let mut stmt = conn.prepare("SELECT run_id FROM orchestration_v2_projection_runs WHERE thread_id = 't-ops' ORDER BY ordinal")?;
        stmt.query_map([], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?
    };
    assert_eq!(runs.len(), 2);
    let cancel: OrchestrationCommand = serde_json::from_value(
        json!({ "type": "queued-run.cancel", "commandId": "c-cancel", "threadId": "t-ops", "runId": runs[1] }),
    )?;
    orch.dispatch_command(&cancel, "2026-11-20T00:02:02.000Z")
        .await?;

    let events: Vec<DomainEvent> = vec![
        serde_json::from_value(json!({
            "id": "e-sub", "threadId": "t-ops", "occurredAt": "2026-11-20T00:02:03.000Z", "type": "subagent.updated",
            "payload": { "id": "node-sa1", "threadId": "t-ops", "runId": runs[0], "parentNodeId": "node-root", "origin": "provider_native", "createdBy": "agent", "driver": "codex", "providerInstanceId": "codex", "providerThreadId": null, "childThreadId": null, "nativeTaskRef": null, "prompt": "Check", "title": "Worker", "model": null, "status": "running", "result": null, "startedAt": "2026-11-20T00:02:03Z", "completedAt": null, "updatedAt": "2026-11-20T00:02:03Z" }
        }))?,
        serde_json::from_value(json!({
            "id": "e-ti", "threadId": "t-ops", "occurredAt": "2026-11-20T00:02:04.000Z", "type": "turn-item.updated",
            "payload": { "type": "assistant_message", "id": "ti-1", "threadId": "t-ops", "runId": runs[0], "nodeId": "node-root", "providerThreadId": null, "providerTurnId": null, "nativeItemRef": null, "parentItemId": null, "ordinal": 1, "status": "completed", "title": null, "startedAt": "2026-11-20T00:02:04Z", "completedAt": "2026-11-20T00:02:04Z", "updatedAt": "2026-11-20T00:02:04Z", "messageId": "m-a1", "text": "Done", "streaming": false }
        }))?,
        serde_json::from_value(json!({
            "id": "e-cp", "threadId": "t-ops", "occurredAt": "2026-11-20T00:02:05.000Z", "type": "checkpoint.captured",
            "payload": { "id": "cp-1", "threadId": "t-ops", "scopeId": "sc-1", "runId": runs[0], "nodeId": "node-root", "parentCheckpointId": null, "ordinalWithinScope": 1, "appRunOrdinal": 1, "ref": "refs/legioncode/checkpoints/cp-1", "status": "ready", "files": [], "capturedAt": "2026-11-20T00:02:05Z" }
        }))?,
        serde_json::from_value(json!({
            "id": "e-req", "threadId": "t-ops", "occurredAt": "2026-11-20T00:02:06.000Z", "type": "runtime-request.updated",
            "payload": { "id": "rq-1", "nodeId": "node-root", "providerTurnId": null, "nativeRequestRef": null, "kind": "approval", "status": "pending", "responseCapability": { "type": "live" }, "createdAt": "2026-11-20T00:02:06Z", "resolvedAt": null }
        }))?,
    ];
    orch.sink().commit_command(&CommitCommandInput {
        command_id: CommandId::new("c-sink-ext")?,
        command_type: "provider.ingest".to_owned(),
        thread_id: ThreadId::new("t-ops")?,
        actor_kind: "provider".to_owned(),
        causation_event_id: None,
        correlation_id: None,
        now_iso: "2026-11-20T00:02:06.000Z".to_owned(),
        events,
        outbox_effects: Vec::new(),
    })?;

    let detail = {
        let conn = orch.sink().db().lock().map_err(|_| "lock poisoned")?;
        query_wire_thread_detail(&conn, "t-ops")?
    };
    assert_eq!(detail.projection.thread["title"], "Renamed");
    assert_eq!(detail.projection.thread["branch"], "feat/wire");
    assert_eq!(detail.projection.thread["runtimeMode"], "approval-required");
    assert_eq!(detail.projection.thread["tags"], json!(["wire", "p2a"]));
    assert!(!detail.projection.thread["autoSettleDisabledAt"].is_null());
    assert_eq!(detail.projection.checkpoints.len(), 1);
    assert_eq!(detail.projection.runtime_requests.len(), 1);
    assert_eq!(detail.projection.subagents.len(), 1);
    assert!(
        detail
            .projection
            .runs
            .iter()
            .all(|r| r.get("status").and_then(Value::as_str) != Some("queued"))
    );
    Ok(())
}
