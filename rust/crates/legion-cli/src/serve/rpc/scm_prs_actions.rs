//! Pull request review guide, viewed-files tracking, and mutation RPC handlers (`pullRequests.*`).

use std::sync::Arc;

use legion_protocol::rpc_codec::RpcExit;
use rusqlite::params;
use serde::Deserialize;
use serde_json::{Value, json};

use super::scm_prs::{PrRefPayload, pr_err, run_gh_text};
use super::{AppState, parse_optional_payload, parse_payload, rpc_fail, utc_now_iso, with_db};

pub(crate) async fn dispatch(state: &Arc<AppState>, method: &str, payload: Value) -> RpcExit {
    match method {
        "reviewGuide" => get_review_guide(state, payload),
        "generateReviewGuide" => generate_review_guide(payload),
        "diffFileContents" => diff_file_contents(payload),
        "filesViewed" => get_files_viewed(state, payload),
        "changeBreakdown" => RpcExit::Success {
            value: json!({ "groups": [] }),
        },
        "setFilesViewed" => set_files_viewed(state, payload),
        "runAction" => run_action(payload).await,
        "update" => update_pr(payload).await,
        "comment" | "replyToThread" => comment_pr(payload).await,
        "updateComment"
        | "setThreadResolution"
        | "setReaction"
        | "invalidate"
        | "reportState"
        | "subscribeLinkedThreadRefreshes"
        | "requestReviewers"
        | "setLabels" => RpcExit::Success { value: json!({}) },
        "reviewerCandidates" | "labelCandidates" => RpcExit::Success {
            value: json!({ "items": [] }),
        },
        _ => rpc_fail(
            "UnsupportedRpcMethod",
            &format!("Unsupported RPC method `pullRequests.{method}`"),
        ),
    }
}

fn get_review_guide(state: &AppState, payload: Value) -> RpcExit {
    let req: PrRefPayload = match parse_optional_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let host = req.host.as_deref().unwrap_or("github.com");
    let repo = req.repository.as_deref().unwrap_or("");
    let num = i64::try_from(req.number.unwrap_or(0)).unwrap_or(0);
    with_db(state, |conn| {
        let raw: Option<String> = conn
            .query_row(
                "SELECT guide_json FROM pull_request_review_guides
                 WHERE host = ?1 AND repository = ?2 AND number = ?3",
                params![host, repo, num],
                |r| r.get(0),
            )
            .ok();
        let guide = raw.and_then(|s| serde_json::from_str::<Value>(&s).ok());
        Ok(RpcExit::Success {
            value: json!({
                "guide": guide,
                "run": Value::Null,
                "currentHeadSha": Value::Null,
            }),
        })
    })
    .unwrap_or_else(|e| e)
}

fn generate_review_guide(payload: Value) -> RpcExit {
    let _: PrRefPayload = match parse_optional_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    RpcExit::Success {
        value: json!({
            "guide": Value::Null,
            "run": Value::Null,
            "currentHeadSha": Value::Null,
        }),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct PrDiffFilePayload {
    path: Option<String>,
}

fn diff_file_contents(payload: Value) -> RpcExit {
    let req: PrDiffFilePayload = match parse_optional_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    RpcExit::Success {
        value: json!({
            "path": req.path.unwrap_or_default(),
            "oldContents": Value::Null,
            "newContents": Value::Null,
            "binary": false,
        }),
    }
}

fn get_files_viewed(state: &AppState, payload: Value) -> RpcExit {
    let req: PrRefPayload = match parse_optional_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let provider = req.provider.as_deref().unwrap_or("github");
    let host = req.host.as_deref().unwrap_or("github.com");
    let repo = req.repository.as_deref().unwrap_or("");
    let viewer = req.viewer.as_deref().unwrap_or("default");
    let num = i64::try_from(req.number.unwrap_or(0)).unwrap_or(0);
    with_db(state, |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT path, revision, viewed_at FROM pull_request_files_viewed
                 WHERE provider = ?1 AND host = ?2 AND repository = ?3 AND number = ?4 AND viewer = ?5",
            )
            .map_err(|e| pr_err(&e.to_string()))?;
        let files: Vec<Value> = stmt
            .query_map(params![provider, host, repo, num, viewer], |r| {
                Ok(json!({
                    "path": r.get::<_, String>(0)?,
                    "revision": r.get::<_, Option<String>>(1)?,
                    "viewedAt": r.get::<_, String>(2)?,
                }))
            })
            .and_then(Iterator::collect)
            .unwrap_or_default();
        Ok(RpcExit::Success {
            value: json!({ "files": files }),
        })
    })
    .unwrap_or_else(|e| e)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetFilesViewedPayload {
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    host: Option<String>,
    #[serde(default)]
    repository: Option<String>,
    number: u64,
    #[serde(default)]
    viewer: Option<String>,
    paths: Vec<String>,
    viewed: bool,
    #[serde(default)]
    revision: Option<String>,
}

fn set_files_viewed(state: &AppState, payload: Value) -> RpcExit {
    let req: SetFilesViewedPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let provider = req.provider.as_deref().unwrap_or("github");
    let host = req.host.as_deref().unwrap_or("github.com");
    let repo = req.repository.as_deref().unwrap_or("");
    let viewer = req.viewer.as_deref().unwrap_or("default");
    let num = i64::try_from(req.number).unwrap_or(0);
    let now = utc_now_iso();
    with_db(state, |conn| {
        for p in &req.paths {
            if req.viewed {
                let _ = conn.execute(
                    "INSERT INTO pull_request_files_viewed
                       (provider, host, repository, number, viewer, path, revision, viewed_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                     ON CONFLICT(provider, host, repository, number, viewer, path)
                     DO UPDATE SET revision = excluded.revision, viewed_at = excluded.viewed_at",
                    params![provider, host, repo, num, viewer, p, req.revision, now],
                );
            } else {
                let _ = conn.execute(
                    "DELETE FROM pull_request_files_viewed
                     WHERE provider = ?1 AND host = ?2 AND repository = ?3 AND number = ?4 AND viewer = ?5 AND path = ?6",
                    params![provider, host, repo, num, viewer, p],
                );
            }
        }
        Ok(RpcExit::Success { value: json!({}) })
    })
    .unwrap_or_else(|e| e)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunActionPayload {
    cwd: Option<String>,
    number: u64,
    action: String,
}

async fn run_action(payload: Value) -> RpcExit {
    let req: RunActionPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let num_str = req.number.to_string();
    let res = match req.action.as_str() {
        "merge" => run_gh_text(req.cwd.as_deref(), &["pr", "merge", &num_str]).await,
        "close" => run_gh_text(req.cwd.as_deref(), &["pr", "close", &num_str]).await,
        "reopen" => run_gh_text(req.cwd.as_deref(), &["pr", "reopen", &num_str]).await,
        "ready" => run_gh_text(req.cwd.as_deref(), &["pr", "ready", &num_str]).await,
        other => Err(format!("Unsupported PR action `{other}`")),
    };
    match res {
        Ok(_) => RpcExit::Success {
            value: json!({ "ok": true }),
        },
        Err(err) => pr_err(&err),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdatePrPayload {
    cwd: Option<String>,
    number: u64,
    title: Option<String>,
    body: Option<String>,
}

async fn update_pr(payload: Value) -> RpcExit {
    let req: UpdatePrPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let num_str = req.number.to_string();
    let mut args = vec!["pr", "edit", &num_str];
    if let Some(ref t) = req.title {
        args.extend(["--title", t]);
    }
    if let Some(ref b) = req.body {
        args.extend(["--body", b]);
    }
    match run_gh_text(req.cwd.as_deref(), &args).await {
        Ok(_) => RpcExit::Success {
            value: json!({ "updated": true }),
        },
        Err(err) => pr_err(&err),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommentPrPayload {
    cwd: Option<String>,
    number: u64,
    body: String,
}

async fn comment_pr(payload: Value) -> RpcExit {
    let req: CommentPrPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let num_str = req.number.to_string();
    match run_gh_text(
        req.cwd.as_deref(),
        &["pr", "comment", &num_str, "--body", &req.body],
    )
    .await
    {
        Ok(_) => RpcExit::Success {
            value: json!({ "posted": true }),
        },
        Err(err) => pr_err(&err),
    }
}
