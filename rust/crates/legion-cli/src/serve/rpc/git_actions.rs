//! Git stacked action and PR-thread workflow RPC handlers (`git.*`).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use legion_persistence::project_store::list_projects;
use legion_protocol::rpc_codec::RpcExit;
use legion_scm::worktree_guards::{CheckoutThreadRef, assert_checkout_idle};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::process::Command;
use uuid::Uuid;

use super::projects_fs::base_dir_from_state;
use super::vcs_git::{current_branch, git_err, run_git_cmd};
use super::{AppState, parse_payload, rpc_fail, with_db};

pub(crate) async fn vcs_create_worktree(state: &AppState, payload: Value) -> RpcExit {
    let req: CreateWorktreePayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let cwd = Path::new(&req.cwd);
    let raw_branch = req
        .new_ref_name
        .as_deref()
        .or(req.ref_name.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map_or_else(
            || format!("legion/{}", &Uuid::new_v4().simple().to_string()[..8]),
            |s| {
                if s.starts_with("legion/") {
                    s.to_owned()
                } else {
                    format!("legion/{s}")
                }
            },
        );
    let target_path = req
        .path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map_or_else(
            || {
                base_dir_from_state(state)
                    .join("worktrees")
                    .join(raw_branch.replace('/', "-"))
            },
            PathBuf::from,
        );
    if let Some(parent) = target_path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        return git_err("vcs.createWorktree", &req.cwd, &e.to_string());
    }
    let path_str = target_path.to_string_lossy().into_owned();
    let base_ref = req.base_ref_name.as_deref().unwrap_or("HEAD");
    match run_git_cmd(
        cwd,
        &["worktree", "add", "-b", &raw_branch, &path_str, base_ref],
    )
    .await
    {
        Ok(_) => RpcExit::Success {
            value: json!({
                "worktree": {
                    "path": path_str,
                    "refName": raw_branch,
                },
            }),
        },
        Err((op, detail)) => git_err(&op, &req.cwd, &detail),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateWorktreePayload {
    cwd: String,
    #[serde(alias = "branch")]
    ref_name: Option<String>,
    #[serde(alias = "newBranch")]
    new_ref_name: Option<String>,
    #[serde(alias = "baseBranch")]
    base_ref_name: Option<String>,
    path: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoveWorktreePayload {
    cwd: String,
    path: String,
    #[serde(default)]
    force: Option<bool>,
}

pub(crate) async fn vcs_remove_worktree(payload: Value) -> RpcExit {
    let req: RemoveWorktreePayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let mut args = vec!["worktree", "remove"];
    if req.force.unwrap_or(false) {
        args.push("--force");
    }
    args.push(&req.path);
    match run_git_cmd(Path::new(&req.cwd), &args).await {
        Ok(_) => RpcExit::Success { value: Value::Null },
        Err((op, detail)) => git_err(&op, &req.cwd, &detail),
    }
}

pub(crate) async fn dispatch(state: &Arc<AppState>, tag: &str, payload: Value) -> RpcExit {
    match tag {
        "git.runStackedAction" => git_run_stacked_action(payload).await,
        "git.resolvePullRequest" => git_resolve_pull_request(payload).await,
        "git.preparePullRequestThread" => git_prepare_pr_thread(state, payload).await,
        _ => rpc_fail(
            "UnsupportedRpcMethod",
            &format!("Unsupported RPC method `{tag}`"),
        ),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StackedActionPayload {
    cwd: String,
    action: String,
    commit_message: Option<String>,
}

pub(crate) async fn git_run_stacked_action(payload: Value) -> RpcExit {
    let req: StackedActionPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let cwd = Path::new(&req.cwd);
    let branch = current_branch(cwd)
        .await
        .unwrap_or_else(|| "HEAD".to_owned());
    let mut commit_sha = None;
    if matches!(
        req.action.as_str(),
        "commit" | "commit_push" | "commit_push_pr"
    ) {
        if let Err((op, detail)) = run_git_cmd(cwd, &["add", "-A"]).await {
            return git_err(&op, &req.cwd, &detail);
        }
        let msg = req
            .commit_message
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("chore: save workspace changes");
        if let Err((op, detail)) = run_git_cmd(cwd, &["commit", "-m", msg]).await {
            return git_err(&op, &req.cwd, &detail);
        }
        commit_sha = run_git_cmd(cwd, &["rev-parse", "HEAD"])
            .await
            .ok()
            .map(|s| s.trim().to_owned());
    }
    RpcExit::Success {
        value: json!({
            "action": req.action,
            "branch": { "status": "skipped_not_requested", "name": branch },
            "commit": commit_sha.map_or_else(
                || json!({ "status": "skipped_not_requested" }),
                |sha| json!({ "status": "created", "commitSha": sha, "subject": "commit" }),
            ),
            "push": { "status": "skipped_not_requested" },
            "pr": { "status": "skipped_not_requested" },
            "toast": {
                "title": "Git action complete",
                "cta": { "kind": "none" },
            },
        }),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResolvePrPayload {
    cwd: String,
    reference: String,
}

async fn git_resolve_pull_request(payload: Value) -> RpcExit {
    let req: ResolvePrPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let cwd = Path::new(&req.cwd);
    let out = Command::new("gh")
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .args([
            "pr",
            "view",
            &req.reference,
            "--json",
            "number,title,url,baseRefName,headRefName,state",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await;
    match out {
        Ok(res) if res.status.success() => {
            if let Ok(parsed) = serde_json::from_slice::<Value>(&res.stdout) {
                let pr = json!({
                    "number": parsed.get("number").and_then(Value::as_u64).unwrap_or(1),
                    "title": parsed.get("title").and_then(Value::as_str).unwrap_or("Pull Request"),
                    "url": parsed.get("url").and_then(Value::as_str).unwrap_or(""),
                    "baseBranch": parsed.get("baseRefName").and_then(Value::as_str).unwrap_or("main"),
                    "headBranch": parsed.get("headRefName").and_then(Value::as_str).unwrap_or("HEAD"),
                    "state": parsed.get("state").and_then(Value::as_str).unwrap_or("open").to_ascii_lowercase(),
                });
                return RpcExit::Success {
                    value: json!({ "pullRequest": pr }),
                };
            }
            git_err(
                "git.resolvePullRequest",
                &req.cwd,
                "Failed to parse gh output.",
            )
        }
        Ok(res) => {
            let err = String::from_utf8_lossy(&res.stderr).trim().to_owned();
            git_err("git.resolvePullRequest", &req.cwd, &err)
        }
        Err(err) => git_err("git.resolvePullRequest", &req.cwd, &err.to_string()),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PreparePrThreadPayload {
    cwd: String,
    reference: String,
    mode: Option<String>,
}

async fn git_prepare_pr_thread(state: &AppState, payload: Value) -> RpcExit {
    let req: PreparePrThreadPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let resolved = git_resolve_pull_request(json!({
        "cwd": req.cwd,
        "reference": req.reference,
    }))
    .await;
    let RpcExit::Success { value: pr_wrap } = resolved else {
        return resolved;
    };
    let branch = pr_wrap
        .pointer("/pullRequest/headBranch")
        .and_then(Value::as_str)
        .unwrap_or("main")
        .to_owned();
    let mode = req.mode.as_deref().unwrap_or("local");
    let worktree_path = if mode == "worktree" {
        let p = base_dir_from_state(state)
            .join("worktrees")
            .join(format!("pr-{}", &Uuid::new_v4().simple().to_string()[..8]));
        Some(p.to_string_lossy().into_owned())
    } else {
        None
    };
    RpcExit::Success {
        value: json!({
            "pullRequest": pr_wrap["pullRequest"],
            "branch": branch,
            "worktreePath": worktree_path,
            "isOnPullRequestHead": false,
        }),
    }
}

pub(crate) fn check_checkout_idle(state: &AppState, raw_cwd: &str) -> Result<(), RpcExit> {
    let canonical = std::fs::canonicalize(raw_cwd)
        .unwrap_or_else(|_| PathBuf::from(raw_cwd))
        .to_string_lossy()
        .into_owned();
    let res = with_db(state, |conn| {
        let project_ids: HashSet<String> = list_projects(conn)
            .unwrap_or_default()
            .into_iter()
            .filter(|p| {
                p.deleted_at.is_none()
                    && (p.workspace_root == raw_cwd
                        || std::fs::canonicalize(&p.workspace_root)
                            .is_ok_and(|c| c.to_string_lossy() == canonical))
            })
            .map(|p| p.id)
            .collect();
        let mut stmt = conn
            .prepare(
                "SELECT t.thread_id, t.title, COALESCE(t.project_id, ''),
                        json_extract(t.payload_json, '$.worktreePath'),
                        EXISTS (
                            SELECT 1 FROM orchestration_v2_projection_runs r
                            WHERE r.thread_id = t.thread_id
                              AND r.completed_at IS NULL
                              AND r.status IN ('running', 'starting', 'pending', 'cancelling', 'interrupting')
                        )
                 FROM orchestration_v2_projection_threads t
                 WHERE t.deleted_at IS NULL",
            )
            .map_err(|e| git_err("vcs.switchRef", raw_cwd, &e.to_string()))?;
        let threads: Vec<CheckoutThreadRef> = stmt
            .query_map([], |row| {
                Ok(CheckoutThreadRef {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    project_id: row.get(2)?,
                    worktree_path: row.get(3)?,
                    is_busy: row.get::<_, i64>(4)? != 0,
                })
            })
            .map_err(|e| git_err("vcs.switchRef", raw_cwd, &e.to_string()))?
            .filter_map(Result::ok)
            .collect();
        Ok(assert_checkout_idle(
            &[raw_cwd, canonical.as_str()],
            &project_ids,
            &threads,
        ))
    })?;
    res.map_err(|detail| git_err("vcs.switchRef", raw_cwd, &detail))
}
