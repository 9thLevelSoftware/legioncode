//! Code review RPC handlers (`review.getBranchCommits`, `review.getDiffPreview`, `review.getDiffFileContents`).

use std::fs;
use std::path::Path;

use legion_protocol::rpc_codec::RpcExit;
use serde::Deserialize;
use serde_json::{Value, json};

use super::vcs_git::{current_branch, run_git_cmd};
use super::{parse_payload, rpc_fail_obj, utc_now_iso};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BranchCommitsPayload {
    cwd: String,
    base_ref: Option<String>,
    limit: Option<usize>,
}

pub(crate) async fn get_branch_commits(payload: Value) -> RpcExit {
    let req: BranchCommitsPayload = match parse_payload(payload) {
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
            value: json!({ "commits": [], "truncated": false }),
        };
    }
    let limit = req.limit.unwrap_or(50).clamp(1, 200);
    let range = req
        .base_ref
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map_or_else(|| "HEAD".to_owned(), |b| format!("{b}..HEAD"));
    let count_flag = format!("-n{}", limit + 1);
    let Ok(out) = run_git_cmd(
        cwd,
        &["log", &count_flag, "--format=%H\t%h\t%s\t%aI\t%an", &range],
    )
    .await
    else {
        return RpcExit::Success {
            value: json!({ "commits": [], "truncated": false }),
        };
    };
    let mut commits = Vec::new();
    let mut truncated = false;
    for line in out.lines() {
        if commits.len() >= limit {
            truncated = true;
            break;
        }
        let mut cols = line.split('\t');
        let sha = cols.next().unwrap_or("");
        let short_sha = cols.next().unwrap_or("");
        let subject = cols.next().unwrap_or("");
        let committed_at = cols.next().unwrap_or("");
        let author = cols.next().unwrap_or("");
        if sha.is_empty() {
            continue;
        }
        commits.push(json!({
            "sha": sha,
            "shortSha": short_sha,
            "subject": subject,
            "committedAt": committed_at,
            "authorName": author,
        }));
    }
    RpcExit::Success {
        value: json!({ "commits": commits, "truncated": truncated }),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiffPreviewPayload {
    cwd: String,
    base_ref: Option<String>,
    commit_sha: Option<String>,
    metadata_only: Option<bool>,
}

pub(crate) async fn get_diff_preview(payload: Value) -> RpcExit {
    let req: DiffPreviewPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let cwd = Path::new(&req.cwd);
    let generated_at = utc_now_iso();
    if !cwd.is_dir()
        || run_git_cmd(cwd, &["rev-parse", "--is-inside-work-tree"])
            .await
            .is_err()
    {
        return RpcExit::Success {
            value: json!({
                "cwd": req.cwd,
                "generatedAt": generated_at,
                "sources": [],
            }),
        };
    }

    let head = current_branch(cwd).await;
    let mut sources = Vec::new();
    if let Some(sha) = req.commit_sha.as_deref().filter(|s| !s.trim().is_empty()) {
        if let Some(files) = collect_diff_files(
            cwd,
            &format!("{sha}^"),
            Some(sha),
            req.metadata_only.unwrap_or(false),
        )
        .await
        {
            sources.push(json!({
                "kind": "commit",
                "commitSha": sha,
                "files": files,
            }));
        }
    } else {
        if let Some(wt_files) =
            collect_diff_files(cwd, "HEAD", None, req.metadata_only.unwrap_or(false)).await
        {
            sources.push(json!({
                "kind": "working-tree",
                "branch": head,
                "files": wt_files,
            }));
        }
        if let Some(base) = req.base_ref.as_deref().filter(|s| !s.trim().is_empty())
            && let Some(branch_files) =
                collect_diff_files(cwd, base, Some("HEAD"), req.metadata_only.unwrap_or(false))
                    .await
        {
            sources.push(json!({
                "kind": "branch",
                "baseRef": base,
                "headRef": head,
                "files": branch_files,
            }));
        }
    }

    RpcExit::Success {
        value: json!({
            "cwd": req.cwd,
            "generatedAt": generated_at,
            "sources": sources,
        }),
    }
}

async fn collect_diff_files(
    cwd: &Path,
    from_ref: &str,
    to_ref: Option<&str>,
    metadata_only: bool,
) -> Option<Vec<Value>> {
    let mut args = vec!["diff", "--numstat"];
    args.push(from_ref);
    if let Some(to) = to_ref {
        args.push(to);
    }
    let numstat = run_git_cmd(cwd, &args).await.ok()?;
    let mut out = Vec::new();
    for line in numstat.lines() {
        let mut parts = line.splitn(3, '\t');
        let add_raw = parts.next().unwrap_or("0");
        let del_raw = parts.next().unwrap_or("0");
        let path = parts.next().unwrap_or("").trim();
        if path.is_empty() {
            continue;
        }
        let binary = add_raw == "-" || del_raw == "-";
        let additions = add_raw.parse::<u64>().unwrap_or(0);
        let deletions = del_raw.parse::<u64>().unwrap_or(0);
        let diff_patch = if metadata_only || binary {
            Value::Null
        } else {
            let mut diff_args = vec!["diff", "--unified=3", from_ref];
            if let Some(to) = to_ref {
                diff_args.push(to);
            }
            diff_args.push("--");
            diff_args.push(path);
            run_git_cmd(cwd, &diff_args)
                .await
                .ok()
                .map_or(Value::Null, Value::String)
        };
        out.push(json!({
            "path": path,
            "additions": additions,
            "deletions": deletions,
            "binary": binary,
            "status": "modified",
            "patch": diff_patch,
        }));
    }
    Some(out)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiffFileContentsPayload {
    cwd: String,
    path: String,
    old_path: Option<String>,
    base_ref: Option<String>,
    head_ref: Option<String>,
}

pub(crate) async fn get_diff_file_contents(payload: Value) -> RpcExit {
    let req: DiffFileContentsPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let cwd = Path::new(&req.cwd);
    if !cwd.is_dir() {
        return rpc_fail_obj(json!({
            "_tag": "VcsRepositoryDetectionError",
            "operation": "ReviewService.getDiffFileContents",
            "cwd": req.cwd,
            "detail": "Workspace directory does not exist.",
            "message": "Workspace directory does not exist.",
        }));
    }
    let old_rev = req.base_ref.as_deref().unwrap_or("HEAD");
    let old_path_rel = req.old_path.as_deref().unwrap_or(&req.path);
    let old_spec = format!("{old_rev}:{old_path_rel}");
    let old_contents = run_git_cmd(cwd, &["show", &old_spec]).await.ok();

    let new_contents = if let Some(head) = req.head_ref.as_deref().filter(|s| !s.trim().is_empty())
    {
        let new_spec = format!("{head}:{}", req.path);
        run_git_cmd(cwd, &["show", &new_spec]).await.ok()
    } else {
        fs::read_to_string(cwd.join(&req.path)).ok()
    };

    RpcExit::Success {
        value: json!({
            "path": req.path,
            "oldPath": req.old_path,
            "oldContents": old_contents,
            "newContents": new_contents,
            "binary": false,
        }),
    }
}
