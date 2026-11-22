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
        "list" => list_prs(payload).await,
        "listStats" => list_stats(payload).await,
        "filterOptions" => RpcExit::Success {
            value: json!({ "authors": [], "labels": [], "repositories": [] }),
        },
        "summary" => summary_pr(payload).await,
        "routing" => routing_pr(payload).await,
        "routingIdentity" => routing_identity(payload).await,
        "stack" => RpcExit::Success {
            value: json!({ "nodes": [], "rootNumber": Value::Null }),
        },
        "linkedThreads" => linked_threads(state, payload),
        "detail" => detail_pr(payload).await,
        "preview" => preview_pr(payload).await,
        "checks" => checks_pr(payload).await,
        "activity" | "threadComments" => activity_pr(payload).await,
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
        "_tag": "PullRequestRpcError",
        "message": detail,
    }))
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ListPrsPayload {
    cwd: Option<String>,
    state: Option<String>,
    limit: Option<usize>,
}

async fn list_prs(payload: Value) -> RpcExit {
    let req: ListPrsPayload = match parse_optional_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let state_arg = req.state.as_deref().unwrap_or("open");
    let limit = req.limit.unwrap_or(30).clamp(1, 100).to_string();
    let args = [
        "pr",
        "list",
        "--state",
        state_arg,
        "--limit",
        &limit,
        "--json",
        "number,title,url,state,isDraft,headRefName,baseRefName,updatedAt,author",
    ];
    match run_gh(req.cwd.as_deref(), &args).await {
        Ok(Value::Array(items)) => RpcExit::Success {
            value: json!({
                "items": items,
                "nextCursor": Value::Null,
            }),
        },
        _ => RpcExit::Success {
            value: json!({ "items": [], "nextCursor": Value::Null }),
        },
    }
}

async fn list_stats(payload: Value) -> RpcExit {
    let req: ListPrsPayload = match parse_optional_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let count = match list_prs(json!({ "cwd": req.cwd, "limit": 50 })).await {
        RpcExit::Success { value } => value
            .get("items")
            .and_then(Value::as_array)
            .map_or(0, Vec::len),
        RpcExit::Failure { .. } => 0,
    };
    RpcExit::Success {
        value: json!({
            "openCount": count,
            "reviewRequestedCount": 0,
            "authoredCount": 0,
        }),
    }
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
    let Some(num) = req.number else {
        return RpcExit::Success { value: Value::Null };
    };
    let num_str = num.to_string();
    let args = [
        "pr",
        "view",
        &num_str,
        "--json",
        "number,title,url,state,isDraft,headRefName,baseRefName,updatedAt,author",
    ];
    match run_gh(req.cwd.as_deref(), &args).await {
        Ok(pr) => RpcExit::Success { value: pr },
        Err(_) => RpcExit::Success { value: Value::Null },
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
            "provider": remote.as_ref().map_or("github", |(p, _, _)| p.as_str()),
            "host": remote.as_ref().map_or("github.com", |(_, h, _)| h.as_str()),
            "repository": remote.as_ref().map_or("", |(_, _, r)| r.as_str()),
            "number": req.number,
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
            "identity": remote.map(|(p, h, r)| json!({
                "provider": p,
                "host": h,
                "repository": r,
            })),
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
                "SELECT thread_id, title, project_id, updated_at
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
        Ok(val) => RpcExit::Success { value: val },
        Err(err) => pr_err(&err),
    }
}

async fn preview_pr(payload: Value) -> RpcExit {
    let req: PrRefPayload = match parse_optional_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let num_str = req.number.unwrap_or(0).to_string();
    let patch = run_gh_text(req.cwd.as_deref(), &["pr", "diff", &num_str])
        .await
        .unwrap_or_default();
    RpcExit::Success {
        value: json!({
            "number": req.number,
            "patch": patch,
            "files": [],
        }),
    }
}

async fn checks_pr(payload: Value) -> RpcExit {
    let req: PrRefPayload = match parse_optional_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let num_str = req.number.unwrap_or(0).to_string();
    match run_gh(
        req.cwd.as_deref(),
        &["pr", "checks", &num_str, "--json", "name,state,bucket,link"],
    )
    .await
    {
        Ok(Value::Array(checks)) => RpcExit::Success {
            value: json!({ "checks": checks }),
        },
        _ => RpcExit::Success {
            value: json!({ "checks": [] }),
        },
    }
}

async fn activity_pr(payload: Value) -> RpcExit {
    let req: PrRefPayload = match parse_optional_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let num_str = req.number.unwrap_or(0).to_string();
    match run_gh(
        req.cwd.as_deref(),
        &["pr", "view", &num_str, "--json", "comments,reviews"],
    )
    .await
    {
        Ok(val) => RpcExit::Success {
            value: json!({
                "items": val.get("comments").cloned().unwrap_or_else(|| json!([])),
                "comments": val.get("comments").cloned().unwrap_or_else(|| json!([])),
                "reviews": val.get("reviews").cloned().unwrap_or_else(|| json!([])),
            }),
        },
        _ => RpcExit::Success {
            value: json!({ "items": [], "comments": [], "reviews": [] }),
        },
    }
}
