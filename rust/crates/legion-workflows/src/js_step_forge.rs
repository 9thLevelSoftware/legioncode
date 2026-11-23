#![forbid(unsafe_code)]
//! Forge (`issues.*` & `pullRequests.*`) step operations and agent step option decoding.

use serde_json::Value;

use crate::engine_agent::AgentStepOptions;
use crate::engine_step::StepExecContext;
use crate::traits::{
    AgentLaunch, AgentModel, IssueAddress, IssueCreateInput, IssueQuery, PullRequestAddress,
    PullRequestQuery,
};

pub(crate) async fn execute_forge_op(
    sctx: &StepExecContext,
    op: &str,
    payload: Value,
) -> Result<Value, String> {
    if op.starts_with("issues.") {
        return execute_issue_op(sctx, op, payload).await;
    }
    if op.starts_with("pullRequests.") {
        return execute_pr_op(sctx, op, payload).await;
    }
    Err(format!("Unknown workflow step operation '{op}'"))
}

async fn execute_issue_op(
    sctx: &StepExecContext,
    op: &str,
    payload: Value,
) -> Result<Value, String> {
    match op {
        "issues.list" => {
            let q: IssueQuery = serde_json::from_value(payload).map_err(|e| e.to_string())?;
            let out = sctx.forge.issues_list(q).await.map_err(|e| e.to_string())?;
            Ok(Value::Array(out))
        }
        "issues.read" => {
            let issue: IssueAddress =
                serde_json::from_value(payload.get("issue").cloned().unwrap_or(Value::Null))
                    .map_err(|e| e.to_string())?;
            sctx.forge
                .issues_read(issue)
                .await
                .map_err(|e| e.to_string())
        }
        "issues.comment" => {
            let issue: IssueAddress =
                serde_json::from_value(payload.get("issue").cloned().unwrap_or(Value::Null))
                    .map_err(|e| e.to_string())?;
            let body = payload
                .get("body")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            sctx.forge
                .issues_comment(issue, body)
                .await
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        "issues.setState" => {
            let issue: IssueAddress =
                serde_json::from_value(payload.get("issue").cloned().unwrap_or(Value::Null))
                    .map_err(|e| e.to_string())?;
            let state = payload
                .get("state")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            sctx.forge
                .issues_set_state(issue, state)
                .await
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        "issues.create" => {
            let input: IssueCreateInput =
                serde_json::from_value(payload).map_err(|e| e.to_string())?;
            sctx.forge
                .issues_create(input)
                .await
                .map_err(|e| e.to_string())
        }
        _ => Err(format!("Unknown workflow issue operation '{op}'")),
    }
}

async fn execute_pr_op(sctx: &StepExecContext, op: &str, payload: Value) -> Result<Value, String> {
    match op {
        "pullRequests.list" => {
            let q: PullRequestQuery = serde_json::from_value(payload).map_err(|e| e.to_string())?;
            let out = sctx
                .forge
                .pull_requests_list(q)
                .await
                .map_err(|e| e.to_string())?;
            Ok(Value::Array(out))
        }
        "pullRequests.read" => {
            let pr: PullRequestAddress =
                serde_json::from_value(payload.get("pr").cloned().unwrap_or(Value::Null))
                    .map_err(|e| e.to_string())?;
            let det = sctx
                .forge
                .pull_requests_read(pr)
                .await
                .map_err(|e| e.to_string())?;
            serde_json::to_value(det).map_err(|e| e.to_string())
        }
        "pullRequests.comment" => {
            let pr: PullRequestAddress =
                serde_json::from_value(payload.get("pr").cloned().unwrap_or(Value::Null))
                    .map_err(|e| e.to_string())?;
            let body = payload
                .get("body")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            sctx.forge
                .pull_requests_comment(pr, body)
                .await
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        "pullRequests.act" => {
            let pr: PullRequestAddress =
                serde_json::from_value(payload.get("pr").cloned().unwrap_or(Value::Null))
                    .map_err(|e| e.to_string())?;
            let action = payload
                .get("action")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let msg = payload
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_owned);
            sctx.forge
                .pull_requests_act(pr, action, msg)
                .await
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        _ => Err(format!("Unknown workflow pull-request operation '{op}'")),
    }
}

pub(crate) fn parse_agent_opts(v: &Value) -> Result<AgentStepOptions, String> {
    let brief = v
        .get("prompt")
        .or_else(|| v.get("brief"))
        .and_then(Value::as_str)
        .ok_or_else(|| "agent step requires a prompt string".to_owned())?
        .to_owned();
    let title = v.get("title").and_then(Value::as_str).map(str::to_owned);
    let returns_schema = v.get("outputSchema").and_then(Value::as_object).cloned();
    let model: Option<AgentModel> = v
        .get("model")
        .filter(|m| !m.is_null())
        .map(|m| serde_json::from_value(m.clone()))
        .transpose()
        .map_err(|e| e.to_string())?;
    let project = v.get("project").and_then(Value::as_str).map(str::to_owned);
    let environment = v
        .get("environment")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let thread = v
        .get("thread")
        .filter(|t| !t.is_null())
        .map(|t| serde_json::from_value(t.clone()))
        .transpose()
        .map_err(|e| e.to_string())?;
    let issue = v
        .get("issue")
        .filter(|i| !i.is_null())
        .map(|i| serde_json::from_value(i.clone()))
        .transpose()
        .map_err(|e| e.to_string())?;
    let launch: Option<AgentLaunch> = v
        .get("launch")
        .filter(|l| !l.is_null())
        .map(|l| serde_json::from_value(l.clone()))
        .transpose()
        .map_err(|e| e.to_string())?;
    let timeout_ms = v.get("timeoutMs").and_then(Value::as_i64);
    let quiet_ms = v.get("quietMs").and_then(Value::as_i64);
    Ok(AgentStepOptions {
        brief,
        title,
        returns_schema,
        model,
        project,
        environment,
        thread,
        issue,
        launch,
        timeout_ms,
        quiet_ms,
    })
}
