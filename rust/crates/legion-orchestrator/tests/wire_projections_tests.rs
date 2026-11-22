//! Integration tests verifying wire projections, decider client ops, and event sink materialization.

mod wire_projections {
    mod thread_ops_tests;
}

use std::collections::HashSet;
use std::error::Error;

use legion_orchestrator::{
    WireOlderSettledCursor, WireShellStreamEvent, build_wire_shell_patch_from_events,
    list_wire_older_settled_threads, query_wire_projects, query_wire_shell_http_snapshot,
    query_wire_shell_stream_reset,
};
use legion_persistence::sqlite::SqliteStore;
use rusqlite::{Connection, params};
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn Error>>;

fn insert_project(
    conn: &Connection,
    id: &str,
    title: &str,
    deleted_at: Option<&str>,
) -> TestResult {
    conn.execute(
        "INSERT INTO projection_projects (
            project_id, title, has_custom_title, workspace_root, default_model_selection_json,
            project_icon_json, scripts_json, created_at, updated_at, deleted_at
        ) VALUES (?1, ?2, 0, ?3, ?4, ?5, '[]', '2026-11-20T00:00:00.000Z', '2026-11-20T00:00:00.000Z', ?6)",
        params![
            id,
            title,
            format!("/tmp/{id}"),
            r#"{"provider":"codex","model":"gpt-5"}"#,
            r#"{"kind":"lucide","monogramText":"AL","color":"slate"}"#,
            deleted_at
        ],
    )?;
    Ok(())
}

fn insert_thread_row(
    conn: &Connection,
    tid: &str,
    pid: &str,
    title: &str,
    updated_at: &str,
    extra: &Value,
) -> TestResult {
    let mut obj = json!({
        "createdBy": "user", "creationSource": "web", "id": tid, "projectId": pid,
        "title": title, "providerInstanceId": "codex",
        "modelSelection": { "provider": "codex", "model": "gpt-5" },
        "runtimeMode": "full-access", "interactionMode": "default",
        "branch": null, "worktreePath": null, "pullRequests": [],
        "lineage": { "parentThreadId": null, "relationshipToParent": null, "rootThreadId": tid },
        "forkedFrom": null, "activeProviderThreadId": null,
        "createdAt": "2026-11-20T00:00:00.000Z", "updatedAt": updated_at,
        "archivedAt": null, "deletedAt": null
    });
    if let (Some(dst), Some(src)) = (obj.as_object_mut(), extra.as_object()) {
        for (k, v) in src {
            dst.insert(k.clone(), v.clone());
        }
    }
    let deleted_at = obj
        .get("deletedAt")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let archived_at = obj
        .get("archivedAt")
        .and_then(Value::as_str)
        .map(str::to_owned);
    conn.execute(
        "INSERT INTO orchestration_v2_projection_threads (
            thread_id, project_id, title, default_provider, runtime_mode, interaction_mode,
            active_provider_thread_id, created_at, updated_at, archived_at, deleted_at, payload_json, provider_instance_id
        ) VALUES (?1, ?2, ?3, 'codex', 'full-access', 'default', NULL, '2026-11-20T00:00:00.000Z', ?4, ?5, ?6, ?7, 'codex')",
        params![tid, pid, title, updated_at, archived_at, deleted_at, serde_json::to_string(&obj)?],
    )?;
    Ok(())
}

fn insert_run_row(
    conn: &Connection,
    rid: &str,
    tid: &str,
    ordinal: i64,
    status: &str,
    requested_at: &str,
) -> TestResult {
    let payload = json!({
        "id": rid, "threadId": tid, "ordinal": ordinal, "status": status,
        "userMessageId": "m1", "modelSelection": { "instanceId": "codex", "model": "gpt-5" },
        "runtimeMode": "full-access", "interactionMode": "default", "requestedAt": requested_at
    });
    conn.execute(
        "INSERT INTO orchestration_v2_projection_runs (
            run_id, thread_id, ordinal, provider, provider_thread_id, status, requested_at, completed_at, payload_json, provider_instance_id
        ) VALUES (?1, ?2, ?3, 'codex', NULL, ?4, ?5, NULL, ?6, 'codex')",
        params![rid, tid, ordinal, status, requested_at, payload.to_string()],
    )?;
    Ok(())
}

fn insert_msg_row(
    conn: &Connection,
    mid: &str,
    tid: &str,
    rid: &str,
    role: &str,
    ts: &str,
    text: &str,
) -> TestResult {
    let payload = json!({
        "id": mid, "threadId": tid, "runId": rid, "role": role, "text": text,
        "createdBy": if role == "user" { "user" } else { "agent" },
        "attachments": [], "createdAt": ts, "updatedAt": ts
    });
    conn.execute(
        "INSERT INTO orchestration_v2_projection_messages (
            message_id, thread_id, run_id, node_id, role, streaming, created_at, updated_at, payload_json, search_text
        ) VALUES (?1, ?2, ?3, NULL, ?4, 0, ?5, ?5, ?6, ?7)",
        params![mid, tid, rid, role, ts, payload.to_string(), text],
    )?;
    Ok(())
}

fn insert_turn_item_row(conn: &Connection, item: &Value) -> TestResult {
    let id = item["id"].as_str().ok_or("item id")?;
    let tid = item["threadId"].as_str().ok_or("item tid")?;
    let rid = item.get("runId").and_then(Value::as_str);
    let ordinal = item["ordinal"].as_i64().ok_or("item ordinal")?;
    let kind = item["type"].as_str().ok_or("item type")?;
    let status = item
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("completed");
    let ts = item["updatedAt"].as_str().ok_or("item ts")?;
    conn.execute(
        "INSERT INTO orchestration_v2_projection_turn_items (
            turn_item_id, thread_id, run_id, node_id, provider_thread_id, provider_turn_id,
            parent_item_id, ordinal, type, status, updated_at, payload_json, inline_work_output
        ) VALUES (?1, ?2, ?3, NULL, NULL, NULL, NULL, ?4, ?5, ?6, ?7, ?8, 0)",
        params![id, tid, rid, ordinal, kind, status, ts, item.to_string()],
    )?;
    Ok(())
}

#[test]
fn wire_projects_and_shell_snapshot_match_effect_wire_contract() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    insert_project(conn, "p-1", "Alpha", None)?;
    insert_project(conn, "p-del", "Deleted", Some("2026-11-20T01:00:00.000Z"))?;

    let projects_snap = query_wire_projects(conn)?;
    let pj = serde_json::to_value(&projects_snap)?;
    assert_eq!(projects_snap.projects.len(), 1);
    let p0 = pj["projects"][0].as_object().ok_or("project obj")?;
    assert!(!p0.contains_key("hasCustomTitle"));
    assert_eq!(p0.get("deletedAt"), Some(&Value::Null));
    assert_eq!(p0["projectIcon"]["kind"], "monogram");
    assert_eq!(p0["projectIcon"]["text"], "AL");
    assert_eq!(p0["defaultModelSelection"]["instanceId"], "codex");
    assert!(p0["defaultModelSelection"].get("provider").is_none());

    for i in 0..27 {
        let tid = format!("t-settled-{i:02}");
        let ts = format!("2026-11-20T00:{i:02}:00.000Z");
        insert_thread_row(
            conn,
            &tid,
            "p-1",
            &format!("Settled {i}"),
            &ts,
            &json!({ "settledAt": ts, "settledOverride": "settled" }),
        )?;
    }
    insert_thread_row(
        conn,
        "t-wait",
        "p-1",
        "Waiting",
        "2026-11-20T02:00:00.000Z",
        &json!({ "settledOverride": "unsettled", "tags": ["urgent"] }),
    )?;
    insert_run_row(
        conn,
        "r-wait",
        "t-wait",
        1,
        "running",
        "2026-11-20T02:00:00.000Z",
    )?;
    insert_turn_item_row(
        conn,
        &json!({
            "id": "ti-wait", "threadId": "t-wait", "runId": "r-wait", "type": "dynamic_tool",
            "toolName": "mcp__legion__legion_thread_wait", "status": "running", "ordinal": 1,
            "createdAt": "2026-11-20T02:00:01.000Z", "updatedAt": "2026-11-20T02:00:01.000Z"
        }),
    )?;

    let http_snap = query_wire_shell_http_snapshot(conn, "env-1")?;
    assert_eq!(http_snap.schema_version, 2);
    assert_eq!(http_snap.archived_threads, []);
    assert!(http_snap.project_identities.is_some());
    let older = http_snap
        .older_settled
        .as_ref()
        .ok_or("olderSettled expected")?;
    assert_eq!(older.counts[0].count, 2);
    let wait_thread = http_snap
        .threads
        .iter()
        .find(|t| t.id == "t-wait")
        .ok_or("t-wait")?;
    assert_eq!(wait_thread.settled_override.as_deref(), Some("active"));
    assert!(wait_thread.blocked_on_thread_wait);
    assert_eq!(wait_thread.model_selection["instanceId"], "codex");
    assert!(wait_thread.model_selection.get("provider").is_none());

    let reset = query_wire_shell_stream_reset(conn, "env-1")?;
    assert!(matches!(reset, WireShellStreamEvent::Snapshot { .. }));

    let cut = WireOlderSettledCursor {
        settled_at: older.before.clone(),
        thread_id: None,
    };
    let total_counts = list_wire_older_settled_threads(conn, None, None, 0)?;
    assert_eq!(total_counts.counts[0].count, 27);
    let counts_page = list_wire_older_settled_threads(conn, None, Some(&cut), 0)?;
    assert_eq!(counts_page.threads, []);
    assert_eq!(counts_page.counts[0].count, 2);
    let page1 = list_wire_older_settled_threads(conn, Some("p-1"), Some(&cut), 1)?;
    assert_eq!(page1.threads.len(), 1);
    assert_eq!(page1.threads[0].id, "t-settled-01");
    Ok(())
}

#[test]
fn wire_shell_patch_coalesces_events_and_respects_prune_gap() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    insert_project(conn, "p-1", "Alpha", None)?;
    insert_thread_row(
        conn,
        "t-main",
        "p-1",
        "Main",
        "2026-11-20T00:00:00.000Z",
        &json!({}),
    )?;
    insert_thread_row(
        conn,
        "t-sub",
        "p-1",
        "Child",
        "2026-11-20T00:00:00.000Z",
        &json!({ "lineage": { "parentThreadId": "t-main", "relationshipToParent": "subagent", "rootThreadId": "t-main" } }),
    )?;
    conn.execute(
        "INSERT INTO orchestration_events (sequence, event_id, aggregate_kind, stream_id, stream_version, event_type, occurred_at, command_id, actor_kind, payload_json, metadata_json, application_event_version)
         VALUES
         (1, 'e1', 'project', 'p-1', 1, 'project.updated', '2026-11-20T00:00:01Z', 'c1', 'system', '{}', '{}', 2),
         (2, 'e2', 'thread', 't-main', 1, 'thread.updated', '2026-11-20T00:00:02Z', 'c2', 'system', '{}', '{}', 2),
         (3, 'e3', 'thread', 't-sub', 1, 'turn-item.updated', '2026-11-20T00:00:03Z', 'c3', 'agent', '{\"type\":\"reasoning\"}', '{}', 2)",
        [],
    )?;
    let mut sub_set = HashSet::from([String::from("t-sub")]);
    let patches = build_wire_shell_patch_from_events(conn, 0, "env-1", Some(&mut sub_set))?;
    assert_eq!(patches.len(), 2);
    assert!(matches!(
        patches[0],
        WireShellStreamEvent::ProjectUpdated { sequence: 1, .. }
    ));
    assert!(matches!(
        patches[1],
        WireShellStreamEvent::ThreadUpdated { sequence: 2, .. }
    ));

    conn.execute(
        "INSERT INTO orchestration_v2_projection_metadata (projection_name, schema_version, last_sequence, updated_at)
         VALUES ('event-log-pruned-through', 2, 2, '2026-11-20T00:00:04Z')",
        [],
    )?;
    let resync = build_wire_shell_patch_from_events(conn, 1, "env-1", None)?;
    assert_eq!(resync, vec![WireShellStreamEvent::SnapshotRequired]);
    Ok(())
}
