//! Issue-tracker connection and project-binding RPC handlers (`issueTrackers.*`).

use legion_protocol::rpc_codec::RpcExit;
use rusqlite::{Connection, params};
use serde::Deserialize;
use serde_json::{Value, json};

use super::super::{AppState, parse_payload, rpc_fail_obj, utc_now_iso, with_db};

fn ensure_tracker_schema(conn: &Connection) {
    let _ = conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS issue_tracker_bindings (
           project_id TEXT PRIMARY KEY,
           tracker TEXT NOT NULL,
           scope TEXT NOT NULL,
           updated_at TEXT NOT NULL
         ) WITHOUT ROWID;",
    );
}

fn tracker_descriptor_json(tracker: &str, connected: bool) -> Value {
    json!({
        "tracker": tracker,
        "displayName": if tracker == "jira" { "Jira" } else { "Linear" },
        "host": if tracker == "jira" { "atlassian.net" } else { "linear.app" },
        "scopeNoun": if tracker == "jira" { "project" } else { "team" },
        "credential": {
            "label": "Personal API key",
            "helpUrl": if tracker == "jira" { Value::Null } else { json!("https://linear.app/settings/api") },
        },
        "connected": connected,
        "account": Value::Null,
        "detail": Value::Null,
        "scopes": [],
    })
}

pub(super) fn list(state: &AppState) -> RpcExit {
    with_db(state, |conn| {
        ensure_tracker_schema(conn);
        let mut stmt = conn
            .prepare("SELECT project_id, tracker, scope FROM issue_tracker_bindings")
            .map_err(|e| {
                rpc_fail_obj(json!({
                    "_tag": "IssueTrackerConnectionError",
                    "tracker": "linear",
                    "reason": "failed",
                    "detail": e.to_string(),
                }))
            })?;
        let mut bindings = serde_json::Map::new();
        let rows: Vec<(String, String, String)> = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .and_then(Iterator::collect)
            .unwrap_or_default();
        for (project_id, tracker, scope) in rows {
            bindings.insert(project_id, json!({ "tracker": tracker, "scope": scope }));
        }
        Ok(RpcExit::Success {
            value: json!({
                "trackers": [tracker_descriptor_json("linear", false)],
                "bindings": bindings,
            }),
        })
    })
    .unwrap_or_else(|e| e)
}

pub(super) fn connect(payload: &Value) -> RpcExit {
    let tracker = payload
        .get("tracker")
        .and_then(Value::as_str)
        .unwrap_or("linear");
    if payload.get("token").is_none_or(Value::is_null) {
        return RpcExit::Success {
            value: tracker_descriptor_json(tracker, false),
        };
    }
    rpc_fail_obj(json!({
        "_tag": "IssueTrackerConnectionError",
        "tracker": tracker,
        "reason": "failed",
        "detail": "Standalone issue tracker credential verification is not configured.",
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TrackerBindingInput {
    tracker: String,
    scope: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BindTrackerPayload {
    project_id: String,
    binding: Option<TrackerBindingInput>,
    tracker: Option<String>,
    scope: Option<String>,
}

pub(super) fn bind_project(state: &AppState, payload: Value) -> RpcExit {
    let req: BindTrackerPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let now = utc_now_iso();
    let tracker_scope = req
        .binding
        .map(|b| (b.tracker, b.scope))
        .or_else(|| req.tracker.zip(req.scope));
    with_db(state, |conn| {
        ensure_tracker_schema(conn);
        if let Some((tracker, scope)) = tracker_scope {
            let _ = conn.execute(
                "INSERT INTO issue_tracker_bindings (project_id, tracker, scope, updated_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(project_id) DO UPDATE SET tracker = excluded.tracker, scope = excluded.scope, updated_at = excluded.updated_at",
                params![req.project_id, tracker, scope, now],
            );
        } else {
            let _ = conn.execute(
                "DELETE FROM issue_tracker_bindings WHERE project_id = ?1",
                params![req.project_id],
            );
        }
        Ok(RpcExit::Success { value: Value::Null })
    })
    .unwrap_or_else(|e| e)
}
