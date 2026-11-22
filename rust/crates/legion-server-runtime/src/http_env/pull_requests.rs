//! `POST /api/pull-requests/diff` handler using `gh pr diff` with paged slicing.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use legion_contracts::permissions::EnvironmentScope;
use legion_persistence::project_store::find_project_by_id_or_workspace;
use serde::Deserialize;
use serde_json::json;
use tokio::process::Command;

use crate::http_api::RuntimeHttpState;
use crate::http_env::{
    EnvErr, EnvResult, authorize_env, env_error_json, env_internal, parse_json_body,
};

const DEFAULT_MAX_DIFF_BYTES: usize = 512 * 1024;
const MAX_ALLOWED_DIFF_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PullRequestDiffRequest {
    project_id: Option<String>,
    cwd: Option<String>,
    host: Option<String>,
    repository: Option<String>,
    number: Option<u64>,
    reference: Option<String>,
    cursor: Option<String>,
    path: Option<String>,
    max_bytes: Option<usize>,
}

fn pr_unavailable(reason: &str, message: impl Into<String>) -> Response {
    env_error_json(
        StatusCode::SERVICE_UNAVAILABLE,
        json!({
            "_tag": "PullRequestUnavailableError",
            "reason": reason,
            "message": message.into(),
        }),
    )
}

fn pr_operation_error(message: impl Into<String>) -> Response {
    env_error_json(
        StatusCode::BAD_GATEWAY,
        json!({
            "_tag": "PullRequestOperationError",
            "message": message.into(),
        }),
    )
}

fn validate_workspace_dir(state: &RuntimeHttpState, raw: &str) -> EnvResult<PathBuf> {
    let canon = fs::canonicalize(Path::new(raw)).map_err(|err| {
        EnvErr(Box::new(pr_operation_error(format!(
            "Invalid workspace path `{raw}`: {err}"
        ))))
    })?;
    if !canon.is_dir() {
        return Err(EnvErr(Box::new(pr_operation_error(format!(
            "Workspace path `{raw}` is not a directory"
        )))));
    }
    if !state.allowed_workspace_roots.is_empty() {
        let allowed = state.allowed_workspace_roots.iter().any(|root| {
            fs::canonicalize(root)
                .map_or_else(|_| canon.starts_with(root), |r| canon.starts_with(&r))
        });
        if !allowed {
            return Err(EnvErr(Box::new(pr_operation_error(
                "Workspace root is outside allowed roots",
            ))));
        }
    }
    Ok(canon)
}

fn resolve_pr_cwd(state: &RuntimeHttpState, req: &PullRequestDiffRequest) -> EnvResult<PathBuf> {
    if let Some(raw_cwd) = req.cwd.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        return validate_workspace_dir(state, raw_cwd);
    }
    let Some(project_id) = req
        .project_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return Err(EnvErr(Box::new(pr_unavailable(
            "repository-without-project",
            "No project or working directory specified for pull request diff",
        ))));
    };

    let conn = state
        .db()
        .lock()
        .map_err(|_| EnvErr(Box::new(env_internal("database lock poisoned"))))?;
    let record = find_project_by_id_or_workspace(&*conn, project_id)
        .map_err(|err| EnvErr(Box::new(env_internal(err.to_string()))))?;
    let Some(proj) = record else {
        return Err(EnvErr(Box::new(pr_unavailable(
            "repository-without-project",
            format!("Project `{project_id}` was not found"),
        ))));
    };
    validate_workspace_dir(state, &proj.workspace_root)
}

fn build_gh_diff_args(req: &PullRequestDiffRequest) -> EnvResult<Vec<String>> {
    let target = req
        .number
        .map(|n| n.to_string())
        .or_else(|| {
            req.reference
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        })
        .ok_or_else(|| {
            EnvErr(Box::new(pr_operation_error(
                "Pull request number or reference is required",
            )))
        })?;

    let mut args = vec!["pr".to_owned(), "diff".to_owned(), target];
    if let Some(repo) = req
        .repository
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let full_repo = match req.host.as_deref().map(str::trim).filter(|h| !h.is_empty()) {
            Some(host) if !repo.contains("://") && !repo.starts_with(host) => {
                format!("{host}/{repo}")
            }
            _ => repo.to_owned(),
        };
        args.push("--repo".to_owned());
        args.push(full_repo);
    }
    Ok(args)
}

fn filter_diff_for_path(diff_text: &str, file_path: &str) -> String {
    let mut selected = String::new();
    let mut active = false;
    for chunk in diff_text.split_inclusive('\n') {
        if chunk.starts_with("diff --git ") {
            active = chunk.contains(file_path);
        }
        if active {
            selected.push_str(chunk);
        }
    }
    selected
}

fn slice_diff_page(
    diff_text: &str,
    cursor: Option<&str>,
    max_bytes: usize,
) -> (String, bool, Option<String>) {
    let offset = cursor
        .and_then(|c| c.strip_prefix("offset:").or(Some(c)))
        .and_then(|n| n.trim().parse::<usize>().ok())
        .unwrap_or(0)
        .min(diff_text.len());
    let mut start = offset;
    while start < diff_text.len() && !diff_text.is_char_boundary(start) {
        start += 1;
    }
    let remaining = &diff_text[start..];
    if remaining.len() <= max_bytes {
        return (remaining.to_owned(), false, None);
    }
    let mut end = max_bytes;
    while end > 0 && !remaining.is_char_boundary(end) {
        end -= 1;
    }
    let next_offset = start + end;
    (
        remaining[..end].to_owned(),
        true,
        Some(format!("offset:{next_offset}")),
    )
}

/// `POST /api/pull-requests/diff` — runs `gh pr diff` and returns a paged `PullRequestDiffResult`.
pub(crate) async fn handle_pull_request_diff(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> EnvResult<Response> {
    authorize_env(&state, &headers, Some(EnvironmentScope::OrchestrationRead))?;
    let req: PullRequestDiffRequest = parse_json_body(&body)?;
    let cwd = resolve_pr_cwd(&state, &req)?;
    let args = build_gh_diff_args(&req)?;

    let output = match Command::new("gh")
        .args(&args)
        .current_dir(&cwd)
        .output()
        .await
    {
        Ok(out) => out,
        Err(err) if err.kind() == ErrorKind::NotFound => {
            return Ok(pr_unavailable(
                "cli-missing",
                "GitHub CLI (`gh`) is not installed or not in PATH",
            ));
        }
        Err(err) => {
            return Ok(pr_operation_error(format!(
                "Failed to run `gh pr diff`: {err}"
            )));
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        let lower = stderr.to_ascii_lowercase();
        if lower.contains("not logged into")
            || lower.contains("gh auth login")
            || lower.contains("authentication")
        {
            return Ok(pr_unavailable("cli-unauthenticated", stderr));
        }
        return Ok(pr_operation_error(if stderr.is_empty() {
            "gh pr diff failed".to_owned()
        } else {
            stderr
        }));
    }

    let raw_patch = String::from_utf8_lossy(&output.stdout).into_owned();
    let patch_text = match req.path.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(file_path) => filter_diff_for_path(&raw_patch, file_path),
        None => raw_patch,
    };
    let max_bytes = req
        .max_bytes
        .unwrap_or(DEFAULT_MAX_DIFF_BYTES)
        .clamp(1024, MAX_ALLOWED_DIFF_BYTES);
    let (slice, truncated, next_cursor) =
        slice_diff_page(&patch_text, req.cursor.as_deref(), max_bytes);

    Ok(Json(json!({
        "patch": slice,
        "truncated": truncated,
        "nextCursor": next_cursor,
        "withheldFilesReadable": req.path.is_none(),
    }))
    .into_response())
}
