//! Authoritative server-side WebSocket RPC scope evaluation and client-only guards (`R5`).

#[path = "server_rules_map.rs"]
mod server_rules_map;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::git_rules::{eval_git_prepare_pr_thread, eval_git_run_stacked_action};
use super::rules::{contains_secret_reference, eval_server_update_settings};
use super::{EnvironmentScope, GuardEvalError, WsMethod, required_scopes_for_project_mutation};

/// Caller classification derived from session subject for human-only RPC enforcement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RpcCallerKind {
    /// Interactive human/client session (browser, desktop, mobile, loopback, CLI client).
    Client,
    /// Thread-scoped, agent MCP, or peer-link agent token (`thread:*`, `mcp-client*`, `mcp-oauth*`, `peer-link*`).
    AgentToken,
}

/// Maps a session `subject` string to [`RpcCallerKind`].
#[must_use]
pub fn caller_kind_from_subject(subject: &str) -> RpcCallerKind {
    let lower = subject.trim().to_ascii_lowercase();
    if lower.starts_with("thread:")
        || lower == "agent"
        || lower.starts_with("agent:")
        || lower == "mcp-client"
        || lower.starts_with("mcp-client:")
        || lower == "mcp-oauth"
        || lower.starts_with("mcp-oauth:")
        || lower.starts_with("mcp-approval:")
        || lower == "peer-link"
        || lower.starts_with("peer-link:")
    {
        RpcCallerKind::AgentToken
    } else {
        RpcCallerKind::Client
    }
}

/// Returns `true` when `method` is a human-only approval/delivery action forbidden for agent tokens.
#[must_use]
pub fn requires_human_client_caller(method: &WsMethod) -> bool {
    matches!(
        method.as_str(),
        "workflowBoard.approveSpec"
            | "workflowBoard.approvePlan"
            | "workflowBoard.approveDelivery"
            | "workflowBoard.returnForRevision"
            | "workflowBoard.resolveFinding"
            | "workflowBoard.recordManualAcceptance"
            | "workflowBoard.ship"
            | "workflowBoard.applyCliImport"
    )
}

/// Builds the `EnvironmentAuthorizationError` JSON payload matching `authScopeRequiredResponse` (`auth.ts:116-124`).
#[must_use]
pub fn auth_scope_required_response(required_permission: EnvironmentScope) -> Value {
    let required_scope = if required_permission == EnvironmentScope::RelayConfigure {
        EnvironmentScope::RelayWrite
    } else {
        required_permission
            .legacy_parent()
            .unwrap_or(required_permission)
    };
    json!({
        "_tag": "EnvironmentAuthorizationError",
        "message": format!(
            "The authenticated token is missing required scope: {}.",
            required_permission.as_str()
        ),
        "requiredScope": required_scope.as_str(),
        "requiredPermission": required_permission.as_str(),
    })
}

/// Builds the `EnvironmentAuthorizationError` JSON payload when an agent token calls a human-only RPC.
#[must_use]
pub fn client_only_required_response() -> Value {
    json!({
        "_tag": "EnvironmentAuthorizationError",
        "message": "This action requires an authenticated human client session; thread and agent tokens are not permitted.",
        "requiredScope": EnvironmentScope::OrchestrationOperate.as_str(),
        "requiredPermission": "client:interactive",
    })
}

/// Evaluates all [`EnvironmentScope`]s required by the server for `method` with optional `input`
/// (`apps/server/src/auth/RpcAuthorization.ts`).
pub fn server_required_scopes_for_rpc(
    method: &WsMethod,
    input: Option<&Value>,
) -> Result<Vec<EnvironmentScope>, GuardEvalError> {
    let normalized;
    let effective = if let WsMethod::Custom(raw) = method
        && let Some(rest) = raw.strip_prefix("orchestration.v2.")
    {
        if rest == "getShellSnapshot" {
            return Ok(vec![EnvironmentScope::OrchestrationRead]);
        }
        normalized = WsMethod::parse(&format!("orchestration.{rest}")).map_err(|_| {
            GuardEvalError::InvalidInput {
                method: raw.clone(),
                reason: "empty normalized orchestration method".to_owned(),
            }
        })?;
        &normalized
    } else {
        method
    };

    let mut scopes = eval_base_server_scopes(effective, input)?;
    if sends_provider_sign_out_prompt(effective, input)
        && !scopes.contains(&EnvironmentScope::ProvidersManage)
    {
        scopes.push(EnvironmentScope::ProvidersManage);
    }
    Ok(scopes)
}

fn eval_base_server_scopes(
    method: &WsMethod,
    input: Option<&Value>,
) -> Result<Vec<EnvironmentScope>, GuardEvalError> {
    match method {
        WsMethod::ServerRetryResourceTelemetry => Ok(vec![
            EnvironmentScope::EnvironmentMaintain,
            EnvironmentScope::DiagnosticsRead,
        ]),
        WsMethod::AssetsCreateUrl => {
            if let Some(payload) = input {
                return Ok(vec![eval_assets_create_url(payload)]);
            }
            Ok(vec![EnvironmentScope::OrchestrationRead])
        }
        WsMethod::ServerUpdateSettings => {
            if let Some(payload) = input {
                return eval_server_update_settings(payload);
            }
            Ok(vec![EnvironmentScope::SettingsWrite])
        }
        WsMethod::DeviceSimlockIdentity
        | WsMethod::DeviceEasAccount
        | WsMethod::ServerListTextGeneratorModels => {
            if input.is_some_and(contains_secret_reference) {
                Ok(vec![
                    EnvironmentScope::SettingsWrite,
                    EnvironmentScope::ProvidersManage,
                ])
            } else {
                Ok(vec![EnvironmentScope::SettingsWrite])
            }
        }
        WsMethod::DeviceList => {
            let needs_operate = input.is_some_and(|val| {
                val.get("retryHostId")
                    .and_then(Value::as_str)
                    .is_some_and(|s| !s.is_empty())
                    || val
                        .get("updateTool")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
            });
            Ok(vec![if needs_operate {
                EnvironmentScope::OrchestrationOperate
            } else {
                EnvironmentScope::OrchestrationRead
            }])
        }
        WsMethod::ProjectsMutate => eval_projects_mutate(input),
        WsMethod::OrchestrationDispatchCommand => eval_dispatch_command(input),
        WsMethod::GitPreparePullRequestThread => {
            if let Some(payload) = input {
                return eval_git_prepare_pr_thread(payload);
            }
            Ok(vec![EnvironmentScope::SourceControlWrite])
        }
        WsMethod::GitRunStackedAction => {
            if let Some(payload) = input {
                return eval_git_run_stacked_action(payload);
            }
            Ok(vec![EnvironmentScope::SourceControlWrite])
        }
        _ => match server_rules_map::static_server_scope(method) {
            Some(scope) => Ok(vec![scope]),
            None => Ok(vec![EnvironmentScope::OrchestrationOperate]),
        },
    }
}

fn eval_projects_mutate(input: Option<&Value>) -> Result<Vec<EnvironmentScope>, GuardEvalError> {
    let Some(payload) = input else {
        return Ok(vec![EnvironmentScope::OrchestrationOperate]);
    };
    let raw_op = payload
        .get("type")
        .and_then(Value::as_str)
        .or_else(|| payload.get("action").and_then(Value::as_str))
        .map_or("project.create", str::trim);
    let normalized_op = if matches!(raw_op, "delete" | "project.delete" | "remove") {
        "project.delete"
    } else {
        raw_op
    };
    required_scopes_for_project_mutation(
        normalized_op,
        payload.get("scripts").filter(|v| !v.is_null()),
    )
}

fn eval_dispatch_command(input: Option<&Value>) -> Result<Vec<EnvironmentScope>, GuardEvalError> {
    if let Some(payload) = input
        && let Some(cmd_type) = payload.get("type").and_then(Value::as_str)
        && matches!(
            cmd_type,
            "project.create"
                | "project.meta.update"
                | "project.update"
                | "project.archive"
                | "project.unarchive"
                | "project.delete"
        )
    {
        return required_scopes_for_project_mutation(
            cmd_type,
            payload.get("scripts").filter(|v| !v.is_null()),
        );
    }
    Ok(vec![EnvironmentScope::OrchestrationOperate])
}

fn eval_assets_create_url(payload: &Value) -> EnvironmentScope {
    let Some(resource) = payload.get("resource") else {
        return EnvironmentScope::OrchestrationRead;
    };
    let tag = resource
        .get("_tag")
        .and_then(Value::as_str)
        .or_else(|| resource.get("kind").and_then(Value::as_str))
        .map(str::trim);
    let app_tag = resource
        .get("app")
        .and_then(|app| {
            app.get("_tag")
                .and_then(Value::as_str)
                .or_else(|| app.get("kind").and_then(Value::as_str))
        })
        .map(str::trim);
    let is_fs = matches!(
        tag,
        Some("workspace-file" | "media-file" | "draft-workspace-file")
    ) || (tag == Some("native-app-icon") && app_tag == Some("app-path"));
    if is_fs {
        EnvironmentScope::FilesystemRead
    } else {
        EnvironmentScope::OrchestrationRead
    }
}

fn is_sign_out_prompt_obj(obj: &Value) -> bool {
    let text_matches = obj
        .get("text")
        .and_then(Value::as_str)
        .is_some_and(|t| t.trim().eq_ignore_ascii_case("/logout"));
    let attachments_empty = obj
        .get("attachments")
        .and_then(Value::as_array)
        .is_none_or(Vec::is_empty);
    text_matches && attachments_empty
}

fn is_turn_start_sign_out_prompt(payload: &Value) -> bool {
    let msg = payload.get("message").and_then(Value::as_object);
    let text_matches = payload
        .get("text")
        .or_else(|| msg.and_then(|m| m.get("text")))
        .and_then(Value::as_str)
        .is_some_and(|t| t.trim().eq_ignore_ascii_case("/logout"));
    let attachments_empty = payload
        .get("attachments")
        .or_else(|| msg.and_then(|m| m.get("attachments")))
        .and_then(Value::as_array)
        .is_none_or(Vec::is_empty);
    text_matches && attachments_empty
}

fn sends_provider_sign_out_prompt(method: &WsMethod, input: Option<&Value>) -> bool {
    let Some(payload) = input else {
        return false;
    };
    match method {
        WsMethod::OrchestrationLaunchThread => payload
            .get("initialMessage")
            .is_some_and(is_sign_out_prompt_obj),
        WsMethod::OrchestrationDispatchCommand => {
            let cmd_type = payload.get("type").and_then(Value::as_str);
            match cmd_type {
                Some("message.dispatch") => is_sign_out_prompt_obj(payload),
                Some("thread.turn.start") => is_turn_start_sign_out_prompt(payload),
                Some("thread.create") => payload
                    .get("initialMessage")
                    .is_some_and(is_sign_out_prompt_obj),
                Some("queued-run.edit") => {
                    payload.get("attachments").is_some_and(Value::is_array)
                        && is_sign_out_prompt_obj(payload)
                }
                Some("delegated_task.request") => payload
                    .get("task")
                    .and_then(Value::as_str)
                    .is_some_and(|t| t.trim().eq_ignore_ascii_case("/logout")),
                _ => false,
            }
        }
        _ => false,
    }
}
