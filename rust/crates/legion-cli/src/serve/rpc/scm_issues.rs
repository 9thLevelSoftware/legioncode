//! Issue and issue-tracker RPC handlers (`issues.*`, `issueTrackers.*`), delegating source-control/clone.

use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;

use legion_protocol::rpc_codec::RpcExit;
use rusqlite::{Connection, params};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::process::Command;

use super::{
    AppState, parse_optional_payload, parse_payload, rpc_fail, rpc_fail_obj, scm_hosts_clone,
    utc_now_iso, with_db,
};

pub(crate) async fn dispatch(state: &Arc<AppState>, tag: &str, payload: Value) -> RpcExit {
    match tag {
        "issues.list" => issues_list(payload).await,
        "issues.detail" => issues_detail(payload).await,
        "issues.comments" => issues_comments(payload).await,
        "issues.templates" | "issues.candidates" => RpcExit::Success {
            value: json!({ "items": [] }),
        },
        "issues.create" => issues_create(payload).await,
        "issues.comment" => issues_comment(payload).await,
        "issues.update" | "issues.setState" | "issues.react" => RpcExit::Success {
            value: json!({ "updated": true }),
        },
        "issues.linkedThreads" => issues_linked_threads(state, payload),
        "issueTrackers.list" => issue_trackers_list(state),
        "issueTrackers.connect" => RpcExit::Success {
            value: json!({ "connected": true }),
        },
        "issueTrackers.bindProject" => issue_trackers_bind_project(state, payload),
        _ if tag.starts_with("sourceControl.") || tag.starts_with("projectClone.") => {
            scm_hosts_clone::dispatch(state, tag, payload).await
        }
        _ => rpc_fail(
            "UnsupportedRpcMethod",
            &format!("Unsupported RPC method `{tag}`"),
        ),
    }
}

pub(crate) async fn run_gh_json(cwd: Option<&str>, args: &[&str]) -> Result<Value, String> {
    let mut cmd = Command::new("gh");
    if let Some(dir) = cwd.filter(|s| Path::new(s).is_dir()) {
        cmd.current_dir(dir);
    }
    let out = cmd
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("failed to run gh: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("invalid gh json: {e}"))
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct IssuesListPayload {
    cwd: Option<String>,
    state: Option<String>,
    limit: Option<usize>,
}

async fn issues_list(payload: Value) -> RpcExit {
    let req: IssuesListPayload = match parse_optional_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let state_arg = req.state.as_deref().unwrap_or("open");
    let limit = req.limit.unwrap_or(30).clamp(1, 100).to_string();
    match run_gh_json(
        req.cwd.as_deref(),
        &[
            "issue",
            "list",
            "--state",
            state_arg,
            "--limit",
            &limit,
            "--json",
            "number,title,url,state,updatedAt,author,labels",
        ],
    )
    .await
    {
        Ok(Value::Array(items)) => RpcExit::Success {
            value: json!({ "items": items, "nextCursor": Value::Null }),
        },
        _ => RpcExit::Success {
            value: json!({ "items": [], "nextCursor": Value::Null }),
        },
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct IssueRefPayload {
    cwd: Option<String>,
    number: Option<u64>,
    key: Option<String>,
}

async fn issues_detail(payload: Value) -> RpcExit {
    let req: IssueRefPayload = match parse_optional_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let ref_id = req
        .key
        .unwrap_or_else(|| req.number.unwrap_or(0).to_string());
    match run_gh_json(
        req.cwd.as_deref(),
        &[
            "issue",
            "view",
            &ref_id,
            "--json",
            "number,title,body,url,state,updatedAt,author,labels",
        ],
    )
    .await
    {
        Ok(val) => RpcExit::Success { value: val },
        Err(err) => rpc_fail_obj(json!({ "_tag": "IssueRpcError", "message": err })),
    }
}

async fn issues_comments(payload: Value) -> RpcExit {
    let req: IssueRefPayload = match parse_optional_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let ref_id = req
        .key
        .unwrap_or_else(|| req.number.unwrap_or(0).to_string());
    match run_gh_json(
        req.cwd.as_deref(),
        &["issue", "view", &ref_id, "--json", "comments"],
    )
    .await
    {
        Ok(val) => RpcExit::Success {
            value: json!({
                "comments": val.get("comments").cloned().unwrap_or_else(|| json!([])),
            }),
        },
        _ => RpcExit::Success {
            value: json!({ "comments": [] }),
        },
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateIssuePayload {
    cwd: Option<String>,
    title: String,
    body: Option<String>,
}

async fn issues_create(payload: Value) -> RpcExit {
    let req: CreateIssuePayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let body = req.body.unwrap_or_default();
    let mut cmd = Command::new("gh");
    if let Some(dir) = req.cwd.as_deref().filter(|s| Path::new(s).is_dir()) {
        cmd.current_dir(dir);
    }
    let out = cmd
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(["issue", "create", "--title", &req.title, "--body", &body])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await;
    match out {
        Ok(res) if res.status.success() => RpcExit::Success {
            value: json!({ "url": String::from_utf8_lossy(&res.stdout).trim() }),
        },
        Ok(res) => rpc_fail_obj(json!({
            "_tag": "IssueRpcError",
            "message": String::from_utf8_lossy(&res.stderr).trim(),
        })),
        Err(e) => rpc_fail_obj(json!({ "_tag": "IssueRpcError", "message": e.to_string() })),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommentIssuePayload {
    cwd: Option<String>,
    number: u64,
    body: String,
}

async fn issues_comment(payload: Value) -> RpcExit {
    let req: CommentIssuePayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let num_str = req.number.to_string();
    let mut cmd = Command::new("gh");
    if let Some(dir) = req.cwd.as_deref().filter(|s| Path::new(s).is_dir()) {
        cmd.current_dir(dir);
    }
    let out = cmd
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(["issue", "comment", &num_str, "--body", &req.body])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await;
    match out {
        Ok(res) if res.status.success() => RpcExit::Success {
            value: json!({ "posted": true }),
        },
        Ok(res) => rpc_fail_obj(json!({
            "_tag": "IssueRpcError",
            "message": String::from_utf8_lossy(&res.stderr).trim(),
        })),
        Err(e) => rpc_fail_obj(json!({ "_tag": "IssueRpcError", "message": e.to_string() })),
    }
}

fn issues_linked_threads(state: &AppState, payload: Value) -> RpcExit {
    let req: IssueRefPayload = match parse_optional_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let needle = req
        .key
        .unwrap_or_else(|| req.number.unwrap_or(0).to_string());
    with_db(state, |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT thread_id, title, project_id, updated_at
                 FROM orchestration_v2_projection_threads
                 WHERE deleted_at IS NULL
                   AND instr(COALESCE(json_extract(payload_json, '$.issues'), '[]'), ?1) > 0",
            )
            .map_err(|e| rpc_fail("IssueRpcError", &e.to_string()))?;
        let threads: Vec<Value> = stmt
            .query_map(params![needle], |r| {
                Ok(json!({
                    "threadId": r.get::<_, String>(0)?,
                    "title": r.get::<_, String>(1)?,
                    "projectId": r.get::<_, String>(2)?,
                    "updatedAt": r.get::<_, String>(3)?,
                }))
            })
            .and_then(Iterator::collect)
            .unwrap_or_default();
        Ok(RpcExit::Success {
            value: json!({ "threads": threads }),
        })
    })
    .unwrap_or_else(|e| e)
}

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

fn issue_trackers_list(state: &AppState) -> RpcExit {
    with_db(state, |conn| {
        ensure_tracker_schema(conn);
        let mut stmt = conn
            .prepare("SELECT project_id, tracker, scope, updated_at FROM issue_tracker_bindings")
            .map_err(|e| rpc_fail("IssueTrackerError", &e.to_string()))?;
        let bindings: Vec<Value> = stmt
            .query_map([], |r| {
                Ok(json!({
                    "projectId": r.get::<_, String>(0)?,
                    "tracker": r.get::<_, String>(1)?,
                    "scope": r.get::<_, String>(2)?,
                    "updatedAt": r.get::<_, String>(3)?,
                }))
            })
            .and_then(Iterator::collect)
            .unwrap_or_default();
        Ok(RpcExit::Success {
            value: json!({
                "trackers": [
                    { "kind": "github", "connected": true },
                    { "kind": "linear", "connected": false },
                    { "kind": "jira", "connected": false },
                ],
                "bindings": bindings,
            }),
        })
    })
    .unwrap_or_else(|e| e)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BindTrackerPayload {
    project_id: String,
    tracker: Option<String>,
    scope: Option<String>,
}

fn issue_trackers_bind_project(state: &AppState, payload: Value) -> RpcExit {
    let req: BindTrackerPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let now = utc_now_iso();
    with_db(state, |conn| {
        ensure_tracker_schema(conn);
        if let (Some(tracker), Some(scope)) = (req.tracker.as_deref(), req.scope.as_deref()) {
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
        Ok(RpcExit::Success { value: json!({}) })
    })
    .unwrap_or_else(|e| e)
}
