//! Pull request read/list RPC handlers (`pullRequests.*`), delegating actions to `scm_prs_actions`.

use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;

use legion_protocol::rpc_codec::RpcExit;
use legion_scm::core::discovery::parse_git_remote_url;
use rusqlite::params;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::process::Command;

use super::{AppState, parse_optional_payload, rpc_fail_obj, scm_prs_actions, with_db};

pub(crate) async fn dispatch(state: &Arc<AppState>, method: &str, payload: Value) -> RpcExit {
    match method {
        "list" => RpcExit::Success {
            value: json!({
                "viewers": {},
                "providers": [],
                "entries": [],
                "errors": [],
                "truncated": false,
                "nextCursors": {},
            }),
        },
        "listStats" => RpcExit::Success {
            value: json!({ "stats": [] }),
        },
        "filterOptions" => RpcExit::Success {
            value: json!({
                "labels": [],
                "authors": [],
                "references": [],
                "truncated": false,
            }),
        },
        "summary" => summary_pr(payload).await,
        "routing" => routing_pr(payload).await,
        "routingIdentity" => routing_identity(payload).await,
        "stack" | "checks" => RpcExit::Success { value: Value::Null },
        "linkedThreads" => linked_threads(state, payload),
        "detail" => detail_pr(payload).await,
        "preview" => preview_pr(payload),
        "activity" => activity_pr(payload),
        "threadComments" => RpcExit::Success {
            value: json!({ "comments": [], "nextCursor": Value::Null, "hiddenCount": 0 }),
        },
        _ => scm_prs_actions::dispatch(state, method, payload).await,
    }
}

pub(crate) async fn run_gh(cwd: Option<&str>, args: &[&str]) -> Result<Value, String> {
    let text = run_gh_text(cwd, args).await?;
    serde_json::from_str(&text).map_err(|e| format!("invalid gh json: {e}"))
}

pub(crate) async fn run_gh_text(cwd: Option<&str>, args: &[&str]) -> Result<String, String> {
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
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub(crate) fn pr_err(detail: &str) -> RpcExit {
    rpc_fail_obj(json!({
        "_tag": "PullRequestOperationError",
        "operation": "pullRequests",
        "detail": if detail.trim().is_empty() { "Pull request unavailable" } else { detail.trim() },
    }))
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct PrRefPayload {
    pub(crate) cwd: Option<String>,
    pub(crate) number: Option<u64>,
    pub(crate) repository: Option<String>,
    pub(crate) host: Option<String>,
    pub(crate) provider: Option<String>,
    pub(crate) viewer: Option<String>,
}

async fn summary_pr(payload: Value) -> RpcExit {
    let req: PrRefPayload = match parse_optional_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let num_str = req.number.unwrap_or(0).to_string();
    let args = [
        "pr",
        "view",
        &num_str,
        "--json",
        "number,title,url,state,isDraft,headRefName,baseRefName,updatedAt,author",
    ];
    match run_gh(req.cwd.as_deref(), &args).await {
        Ok(_) => pr_err("Pull request summary conversion unavailable"),
        Err(err) => pr_err(&err),
    }
}

async fn routing_pr(payload: Value) -> RpcExit {
    let req: PrRefPayload = match parse_optional_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let remote = detect_remote(req.cwd.as_deref()).await;
    RpcExit::Success {
        value: json!({
            "accountId": "default",
            "provider": remote.as_ref().map_or("github", |(p, _, _)| p.as_str()),
            "host": remote.as_ref().map_or("github.com", |(_, h, _)| h.as_str()),
            "viewer": "unknown",
            "projectTitle": "Workspace",
            "workspaceRoot": req.cwd.as_deref().unwrap_or("."),
        }),
    }
}

async fn routing_identity(payload: Value) -> RpcExit {
    let req: PrRefPayload = match parse_optional_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let remote = detect_remote(req.cwd.as_deref()).await;
    RpcExit::Success {
        value: json!({
            "accountId": "default",
            "host": remote.as_ref().map_or("github.com", |(_, h, _)| h.as_str()),
            "provider": remote.as_ref().map_or("github", |(p, _, _)| p.as_str()),
            "viewer": "unknown",
        }),
    }
}

async fn detect_remote(cwd: Option<&str>) -> Option<(String, String, String)> {
    let dir = cwd.filter(|s| Path::new(s).is_dir())?;
    let out = Command::new("git")
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(["remote", "get-url", "origin"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let url = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    let parsed = parse_git_remote_url(&url).ok()?;
    let provider = parsed.provider_hint.map_or_else(
        || "github".to_owned(),
        |kind| format!("{kind:?}").to_ascii_lowercase(),
    );
    Some((provider, parsed.host, parsed.full_name))
}

fn linked_threads(state: &AppState, payload: Value) -> RpcExit {
    let req: PrRefPayload = match parse_optional_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let Some(num) = req.number else {
        return RpcExit::Success {
            value: json!({ "threads": [] }),
        };
    };
    with_db(state, |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT thread_id, project_id, title, archived_at,
                        json_extract(payload_json, '$.settledOverride')
                 FROM orchestration_v2_projection_threads
                 WHERE deleted_at IS NULL
                   AND (
                     CAST(json_extract(payload_json, '$.linkedPullRequest.number') AS INTEGER) = ?1
                     OR instr(COALESCE(json_extract(payload_json, '$.pullRequests'), '[]'), ?2) > 0
                   )",
            )
            .map_err(|e| pr_err(&e.to_string()))?;
        let needle = format!("\"number\":{num}");
        let num_i64 = i64::try_from(num).unwrap_or(0);
        let rows: Vec<Value> = stmt
            .query_map(params![num_i64, needle], |r| {
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
            value: json!({ "threads": rows }),
        })
    })
    .unwrap_or_else(|e| e)
}

async fn detail_pr(payload: Value) -> RpcExit {
    let req: PrRefPayload = match parse_optional_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let num_str = req.number.unwrap_or(0).to_string();
    let args = [
        "pr",
        "view",
        &num_str,
        "--json",
        "number,title,body,url,state,isDraft,headRefName,baseRefName,mergeable,reviewDecision,additions,deletions,changedFiles,author,files",
    ];
    match run_gh(req.cwd.as_deref(), &args).await {
        Ok(_) => pr_err("Pull request detail unavailable"),
        Err(err) => pr_err(&err),
    }
}

fn preview_pr(payload: Value) -> RpcExit {
    let req: PrRefPayload = match parse_optional_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let _ = req;
    pr_err("Pull request preview unavailable")
}

fn activity_pr(payload: Value) -> RpcExit {
    let req: PrRefPayload = match parse_optional_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let _ = req;
    RpcExit::Success {
        value: json!({
            "author": Value::Null,
            "reviewers": [],
            "comments": [],
            "commentCount": 0,
            "commentsTruncated": false,
            "reviewThreadsTruncated": false,
            "reviewThreads": [],
            "overviewThreads": [],
            "commits": [],
            "reactions": [],
        }),
    }
}
