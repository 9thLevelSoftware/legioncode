//! VCS RPC handlers (`vcs.*`, `worktreeSetup.cancel`, `review.*`, and delegation to `git.*`).

use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;

use legion_protocol::rpc_codec::RpcExit;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::process::Command;

use super::{AppState, git_actions, parse_payload, rpc_fail, rpc_fail_obj, vcs_review};

pub(crate) async fn dispatch(state: &Arc<AppState>, tag: &str, payload: Value) -> RpcExit {
    match tag {
        "vcs.pull" => vcs_pull(payload).await,
        "vcs.refreshStatus" => vcs_refresh_status(payload).await,
        "vcs.refreshBranch" => vcs_refresh_branch(payload).await,
        "vcs.listRefs" => vcs_list_refs(payload).await,
        "vcs.createWorktree" => git_actions::vcs_create_worktree(state, payload).await,
        "vcs.removeWorktree" => git_actions::vcs_remove_worktree(payload).await,
        "vcs.createRef" => vcs_create_ref(payload).await,
        "vcs.switchRef" => vcs_switch_ref(payload).await,
        "vcs.init" => vcs_init(payload).await,
        "worktreeSetup.cancel" => RpcExit::Success {
            value: json!({ "cancelled": false }),
        },
        _ if tag.starts_with("git.") => git_actions::dispatch(state, tag, payload).await,
        "review.getBranchCommits" => vcs_review::get_branch_commits(payload).await,
        "review.getDiffPreview" => vcs_review::get_diff_preview(payload).await,
        "review.getDiffFileContents" => vcs_review::get_diff_file_contents(payload).await,
        _ => rpc_fail(
            "UnsupportedRpcMethod",
            &format!("Unsupported RPC method `{tag}`"),
        ),
    }
}

pub(crate) async fn run_git_cmd(cwd: &Path, args: &[&str]) -> Result<String, (String, String)> {
    let out = Command::new("git")
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "")
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| (format!("git {}", args.join(" ")), e.to_string()))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        Err((
            format!("git {}", args.join(" ")),
            if stderr.is_empty() { stdout } else { stderr },
        ))
    }
}

pub(crate) fn git_err(op: &str, cwd: &str, detail: &str) -> RpcExit {
    rpc_fail_obj(json!({
        "_tag": "GitCommandError",
        "operation": op,
        "cwd": cwd,
        "detail": detail,
        "message": format!("{op} failed: {detail}"),
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CwdPayload {
    cwd: String,
}

async fn vcs_pull(payload: Value) -> RpcExit {
    let req: CwdPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let cwd = Path::new(&req.cwd);
    let branch = current_branch(cwd).await.unwrap_or_default();
    match run_git_cmd(cwd, &["pull", "--ff-only"]).await {
        Ok(stdout) => RpcExit::Success {
            value: json!({
                "branch": branch,
                "upstreamBranch": Value::Null,
                "pulled": true,
                "output": stdout.trim(),
            }),
        },
        Err((op, detail)) => git_err(&op, &req.cwd, &detail),
    }
}

async fn vcs_refresh_status(payload: Value) -> RpcExit {
    let req: CwdPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let cwd = Path::new(&req.cwd);
    if !cwd.is_dir()
        || run_git_cmd(cwd, &["rev-parse", "--is-inside-work-tree"])
            .await
            .is_err()
    {
        return RpcExit::Success {
            value: json!({
                "isRepo": false,
                "hasOriginRemote": false,
                "isDefaultBranch": false,
                "branch": Value::Null,
                "hasWorkingTreeChanges": false,
                "workingTree": { "files": [], "insertions": 0, "deletions": 0 },
                "hasUpstream": false,
                "aheadCount": 0,
                "behindCount": 0,
                "pr": Value::Null,
            }),
        };
    }
    let branch = current_branch(cwd).await;
    let has_origin = run_git_cmd(cwd, &["remote", "get-url", "origin"])
        .await
        .is_ok();
    let files = collect_status_files(cwd).await;
    let (has_upstream, ahead, behind) = match run_git_cmd(
        cwd,
        &["rev-list", "--left-right", "--count", "@{upstream}...HEAD"],
    )
    .await
    {
        Ok(txt) => {
            let mut parts = txt.split_whitespace();
            let b = parts
                .next()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0);
            let a = parts
                .next()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0);
            (true, a, b)
        }
        Err(_) => (false, 0, 0),
    };
    let is_default = matches!(branch.as_deref(), Some("main" | "master"));
    RpcExit::Success {
        value: json!({
            "isRepo": true,
            "hasOriginRemote": has_origin,
            "isDefaultBranch": is_default,
            "branch": branch,
            "hasWorkingTreeChanges": !files.is_empty(),
            "workingTree": { "files": files, "insertions": 0, "deletions": 0 },
            "hasUpstream": has_upstream,
            "aheadCount": ahead,
            "behindCount": behind,
            "pr": Value::Null,
        }),
    }
}

async fn collect_status_files(cwd: &Path) -> Vec<Value> {
    let status_out = run_git_cmd(cwd, &["status", "--porcelain=v1", "-z"])
        .await
        .unwrap_or_default();
    let mut files = Vec::new();
    for entry in status_out.split('\0').filter(|s| s.len() >= 4) {
        let code = &entry[..2];
        let path = entry[3..].trim();
        if !path.is_empty() {
            files.push(json!({
                "path": path,
                "status": code.trim(),
                "insertions": 0,
                "deletions": 0,
            }));
        }
    }
    files
}

async fn vcs_refresh_branch(payload: Value) -> RpcExit {
    let req: CwdPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let branch = current_branch(Path::new(&req.cwd)).await;
    RpcExit::Success {
        value: json!({ "branch": branch }),
    }
}

async fn vcs_list_refs(payload: Value) -> RpcExit {
    let req: CwdPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let cwd = Path::new(&req.cwd);
    let current = current_branch(cwd).await;
    let Ok(out) = run_git_cmd(
        cwd,
        &[
            "for-each-ref",
            "--format=%(refname:short)\t%(refname)\t%(HEAD)",
            "refs/heads",
            "refs/remotes",
        ],
    )
    .await
    else {
        return RpcExit::Success {
            value: json!({ "branches": [], "tags": [] }),
        };
    };
    let mut branches = Vec::new();
    for line in out.lines() {
        let mut cols = line.split('\t');
        let short = cols.next().unwrap_or("").trim();
        let full = cols.next().unwrap_or("").trim();
        let head_marker = cols.next().unwrap_or("").trim();
        if short.is_empty() || full.ends_with("/HEAD") {
            continue;
        }
        let is_remote = full.starts_with("refs/remotes/");
        let is_current = !is_remote && (head_marker == "*" || current.as_deref() == Some(short));
        let is_default = matches!(short, "main" | "master");
        branches.push(json!({
            "name": short,
            "current": is_current,
            "isDefault": is_default,
            "isRemote": is_remote,
        }));
    }
    RpcExit::Success {
        value: json!({ "branches": branches, "tags": [] }),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateRefPayload {
    cwd: String,
    branch: String,
}

async fn vcs_create_ref(payload: Value) -> RpcExit {
    let req: CreateRefPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    match run_git_cmd(Path::new(&req.cwd), &["branch", &req.branch]).await {
        Ok(_) => RpcExit::Success {
            value: json!({ "branch": req.branch }),
        },
        Err((op, detail)) => git_err(&op, &req.cwd, &detail),
    }
}

async fn vcs_switch_ref(payload: Value) -> RpcExit {
    let req: CreateRefPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    match run_git_cmd(Path::new(&req.cwd), &["checkout", &req.branch]).await {
        Ok(_) => RpcExit::Success {
            value: json!({ "branch": req.branch }),
        },
        Err((op, detail)) => git_err(&op, &req.cwd, &detail),
    }
}

async fn vcs_init(payload: Value) -> RpcExit {
    let req: CwdPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    match run_git_cmd(Path::new(&req.cwd), &["init"]).await {
        Ok(_) => RpcExit::Success { value: json!({}) },
        Err((op, detail)) => git_err(&op, &req.cwd, &detail),
    }
}

pub(crate) async fn current_branch(cwd: &Path) -> Option<String> {
    let txt = run_git_cmd(cwd, &["rev-parse", "--abbrev-ref", "HEAD"])
        .await
        .ok()?;
    let trimmed = txt.trim();
    if trimmed.is_empty() || trimmed == "HEAD" {
        None
    } else {
        Some(trimmed.to_owned())
    }
}
