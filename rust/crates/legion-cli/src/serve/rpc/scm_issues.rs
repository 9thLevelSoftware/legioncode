//! Issue and issue-tracker RPC handlers (`issues.*`, `issueTrackers.*`), delegating source-control/clone.

use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;

use legion_protocol::rpc_codec::RpcExit;
use rusqlite::params;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::process::Command;

#[path = "scm_issue_trackers.rs"]
mod trackers;

use super::{
    AppState, parse_optional_payload, parse_payload, rpc_fail, rpc_fail_obj, scm_hosts_clone,
    with_db,
};

pub(crate) async fn dispatch(state: &Arc<AppState>, tag: &str, payload: Value) -> RpcExit {
    match tag {
        "issues.list" => RpcExit::Success {
            value: json!({
                "viewers": {},
                "trackers": [],
                "entries": [],
                "errors": [],
                "nextCursors": {},
                "scopes": [],
            }),
        },
        "issues.detail" => issues_detail(payload).await,
        "issues.comments" => issues_comments(payload).await,
        "issues.templates" => RpcExit::Success {
            value: json!({
                "templates": [],
                "externalTemplates": [],
                "blankIssuesEnabled": true,
            }),
        },
        "issues.candidates" => RpcExit::Success {
            value: json!({ "labels": [], "assignees": [], "truncated": false }),
        },
        "issues.create" => issues_create(payload).await,
        "issues.comment" => issues_comment(payload).await,
        "issues.update" | "issues.setState" | "issues.react" => {
            RpcExit::Success { value: Value::Null }
        }
        "issues.linkedThreads" => issues_linked_threads(state, payload),
        "issueTrackers.list" => trackers::list(state),
        "issueTrackers.connect" => trackers::connect(&payload),
        "issueTrackers.bindProject" => trackers::bind_project(state, payload),
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
        Ok(_) => rpc_fail_obj(json!({
            "_tag": "IssueUnavailableError",
            "reason": "tracker-unsupported",
        })),
        Err(err) => rpc_fail_obj(json!({
            "_tag": "IssueOperationError",
            "operation": "issues.detail",
            "detail": err,
        })),
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
        Ok(_) | Err(_) => RpcExit::Success {
            value: json!({ "comments": [], "nextCursor": Value::Null }),
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

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct IssueRefPayload {
    cwd: Option<String>,
    number: Option<u64>,
    key: Option<String>,
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
        Ok(res) if res.status.success() => {
            let url = String::from_utf8_lossy(&res.stdout).trim().to_owned();
            let key = url.rsplit('/').next().unwrap_or("1").to_owned();
            RpcExit::Success {
                value: json!({
                    "key": key,
                    "displayKey": format!("#{key}"),
                    "url": url,
                }),
            }
        }
        Ok(res) => rpc_fail_obj(json!({
            "_tag": "IssueOperationError",
            "operation": "issues.create",
            "detail": String::from_utf8_lossy(&res.stderr).trim(),
        })),
        Err(e) => rpc_fail_obj(json!({
            "_tag": "IssueUnavailableError",
            "reason": "tool-missing",
            "cause": e.to_string(),
        })),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommentIssuePayload {
    cwd: Option<String>,
    #[serde(default)]
    number: Option<u64>,
    #[serde(default)]
    key: Option<String>,
    body: String,
}

async fn issues_comment(payload: Value) -> RpcExit {
    let req: CommentIssuePayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let num_str = req
        .key
        .unwrap_or_else(|| req.number.unwrap_or(0).to_string());
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
        Ok(res) if res.status.success() => RpcExit::Success { value: Value::Null },
        Ok(res) => rpc_fail_obj(json!({
            "_tag": "IssueOperationError",
            "operation": "issues.comment",
            "detail": String::from_utf8_lossy(&res.stderr).trim(),
        })),
        Err(e) => rpc_fail_obj(json!({
            "_tag": "IssueUnavailableError",
            "reason": "tool-missing",
            "cause": e.to_string(),
        })),
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
                "SELECT thread_id, project_id, title, archived_at,
                        json_extract(payload_json, '$.settledOverride')
                 FROM orchestration_v2_projection_threads
                 WHERE deleted_at IS NULL
                   AND instr(COALESCE(json_extract(payload_json, '$.issues'), '[]'), ?1) > 0",
            )
            .map_err(|e| {
                rpc_fail_obj(json!({
                    "_tag": "IssueOperationError",
                    "operation": "issues.linkedThreads",
                    "detail": e.to_string(),
                }))
            })?;
        let threads: Vec<Value> = stmt
            .query_map(params![needle], |r| {
                let settled_str: Option<String> = r.get(4)?;
                Ok(json!({
                    "id": r.get::<_, String>(0)?,
                    "projectId": r.get::<_, String>(1)?,
                    "title": r.get::<_, String>(2)?,
                    "archivedAt": r.get::<_, Option<String>>(3)?,
                    "settled": settled_str.as_deref() == Some("settled"),
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
