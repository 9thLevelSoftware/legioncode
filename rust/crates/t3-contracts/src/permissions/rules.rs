//! Static scope map and dynamic settings payload validation rules for guarded RPCs.

use serde_json::{Map, Value};

use crate::ids::{ProviderDriverKind, ProviderInstanceId};

use super::{EnvironmentScope, GuardEvalError, WsMethod};

pub(super) fn static_guarded_scope(method: &WsMethod) -> Option<EnvironmentScope> {
    match method {
        WsMethod::ProviderAuthRemoveMethod => Some(EnvironmentScope::ProvidersManage),
        WsMethod::PreviewClose => Some(EnvironmentScope::PreviewOperate),
        WsMethod::PreviewStopLocalServer => Some(EnvironmentScope::TerminalOperate),
        WsMethod::ServerRunStorageCleanup
        | WsMethod::ServerUpdateSettings
        | WsMethod::ServerCheckOtlpEndpoint
        | WsMethod::ServerCheckSecretSource
        | WsMethod::ServerListTextGeneratorModels
        | WsMethod::IssueTrackersConnect
        | WsMethod::IssueTrackersBindProject
        | WsMethod::SourceControlSignInStart
        | WsMethod::SourceControlSignInWait
        | WsMethod::SourceControlSignInCancel => Some(EnvironmentScope::SettingsWrite),
        WsMethod::ServerReclaimDatabaseSpace => Some(EnvironmentScope::EnvironmentMaintain),
        WsMethod::ServerRefreshProviders => Some(EnvironmentScope::OrchestrationRead),
        WsMethod::PullRequestsRunAction
        | WsMethod::PullRequestsUpdate
        | WsMethod::PullRequestsComment
        | WsMethod::PullRequestsUpdateComment
        | WsMethod::PullRequestsSubmitReview
        | WsMethod::PullRequestsReplyToThread
        | WsMethod::PullRequestsSetThreadResolution
        | WsMethod::PullRequestsSetReaction
        | WsMethod::PullRequestsSetFilesViewed
        | WsMethod::PullRequestsRequestReviewers
        | WsMethod::PullRequestsSetLabels
        | WsMethod::IssuesCreate
        | WsMethod::IssuesComment
        | WsMethod::IssuesUpdate
        | WsMethod::IssuesSetState
        | WsMethod::IssuesReact
        | WsMethod::SourceControlCloneRepository
        | WsMethod::SourceControlPublishRepository
        | WsMethod::SourceControlStar
        | WsMethod::ProjectCloneStart
        | WsMethod::ProjectCloneCancel
        | WsMethod::ProjectCloneRetry
        | WsMethod::VcsPull
        | WsMethod::GitRunStackedAction
        | WsMethod::GitPreparePullRequestThread
        | WsMethod::VcsCreateWorktree
        | WsMethod::VcsRemoveWorktree
        | WsMethod::VcsCreateRef
        | WsMethod::VcsSwitchRef
        | WsMethod::VcsInit => Some(EnvironmentScope::SourceControlWrite),
        WsMethod::PullRequestsGenerateReviewGuide
        | WsMethod::ProjectsCreateFromThread
        | WsMethod::ServerPrepareTriage
        | WsMethod::ScheduledTasksUpsert
        | WsMethod::ScheduledTasksSetEnabled
        | WsMethod::ScheduledTasksDelete
        | WsMethod::ScheduledTasksRunNow
        | WsMethod::ScheduledTasksRotateWebhookToken
        | WsMethod::OrchestrationStopWorkflow
        | WsMethod::OrchestrationForceStopThread => Some(EnvironmentScope::OrchestrationOperate),
        WsMethod::ProjectsWriteFile => Some(EnvironmentScope::FilesystemWrite),
        WsMethod::GuestShareCreate
        | WsMethod::PeerLinksLink
        | WsMethod::PeerLinksUnlink
        | WsMethod::PeerLinksAnswerRequest => Some(EnvironmentScope::AccessWrite),
        _ => None,
    }
}

fn is_valid_secret_provider_slug(raw: &str) -> bool {
    let trimmed = raw.trim();
    !trimmed.is_empty()
        && trimmed.chars().count() <= 64
        && trimmed
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

fn is_valid_secret_reference_path(raw: &str) -> bool {
    let trimmed = raw.trim();
    !trimmed.is_empty()
        && trimmed.chars().count() <= 1024
        && !trimmed.contains('\r')
        && !trimmed.contains('\n')
}

fn is_secret_reference(value: &Value) -> bool {
    let Some(obj) = value.as_object() else {
        return false;
    };
    let Some(provider) = obj.get("provider").and_then(Value::as_str) else {
        return false;
    };
    let Some(reference) = obj.get("reference").and_then(Value::as_str) else {
        return false;
    };
    is_valid_secret_provider_slug(provider) && is_valid_secret_reference_path(reference)
}

/// Returns `true` if `value` recursively contains a `SecretReference` object.
pub fn contains_secret_reference(value: &Value) -> bool {
    match value {
        Value::Array(items) => items.iter().any(contains_secret_reference),
        Value::Object(map) => {
            is_secret_reference(value) || map.values().any(contains_secret_reference)
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => false,
    }
}

/// Computes scopes required for a validated `ServerSettingsPatch` object (`settings.ts:2001-2034`).
pub fn required_scopes_for_server_settings_patch(
    patch: &Map<String, Value>,
) -> Result<Vec<EnvironmentScope>, GuardEvalError> {
    if let Some(secret_providers) = patch.get("secretProviders") {
        let Some(entries) = secret_providers.as_object() else {
            return Err(GuardEvalError::InvalidInput {
                method: "server.updateSettings".to_owned(),
                reason: "`patch.secretProviders` must be an object".to_owned(),
            });
        };
        for entry in entries.values() {
            if !entry.is_null() && !entry.is_object() {
                return Err(GuardEvalError::InvalidInput {
                    method: "server.updateSettings".to_owned(),
                    reason: "`patch.secretProviders` values must be null or objects".to_owned(),
                });
            }
        }
    }

    let mut changes_providers = false;
    let mut changes_settings = false;
    let mut changes_access = false;

    for key in patch.keys() {
        if key == "requireConnectDeviceApproval" {
            changes_access = true;
        } else if matches!(
            key.as_str(),
            "providers"
                | "providerInstances"
                | "usageLimitSources"
                | "textGenerators"
                | "secretProviders"
        ) {
            changes_providers = true;
        } else {
            changes_settings = true;
        }
    }

    if patch.values().any(contains_secret_reference) {
        changes_providers = true;
    }
    if patch
        .get("secretProviders")
        .and_then(Value::as_object)
        .is_some_and(|entries| entries.values().any(Value::is_null))
    {
        changes_settings = true;
    }

    let mut scopes = Vec::new();
    if changes_settings || (!changes_providers && !changes_access) {
        scopes.push(EnvironmentScope::SettingsWrite);
    }
    if changes_providers {
        scopes.push(EnvironmentScope::ProvidersManage);
    }
    if changes_access {
        scopes.push(EnvironmentScope::AccessWrite);
    }
    Ok(scopes)
}

fn validate_provider_instance_mutation(
    mutation: &Value,
    method: &str,
) -> Result<(), GuardEvalError> {
    let Some(obj) = mutation.as_object() else {
        return Err(GuardEvalError::InvalidInput {
            method: method.to_owned(),
            reason: "`providerInstanceMutation` must be an object".to_owned(),
        });
    };
    let Some(op) = obj.get("operation").and_then(Value::as_str) else {
        return Err(GuardEvalError::InvalidInput {
            method: method.to_owned(),
            reason: "`providerInstanceMutation.operation` must be a string".to_owned(),
        });
    };
    let instance_id = obj
        .get("instanceId")
        .and_then(Value::as_str)
        .ok_or_else(|| GuardEvalError::InvalidInput {
            method: method.to_owned(),
            reason: "`providerInstanceMutation.instanceId` must be a string".to_owned(),
        })?;
    ProviderInstanceId::new(instance_id).map_err(|err| GuardEvalError::InvalidInput {
        method: method.to_owned(),
        reason: format!("invalid `providerInstanceMutation.instanceId`: {err}"),
    })?;

    match op {
        "create" | "upsert" => {
            let Some(instance) = obj.get("instance").and_then(Value::as_object) else {
                return Err(GuardEvalError::InvalidInput {
                    method: method.to_owned(),
                    reason: "`providerInstanceMutation.instance` must be an object".to_owned(),
                });
            };
            let driver = instance
                .get("driver")
                .and_then(Value::as_str)
                .ok_or_else(|| GuardEvalError::InvalidInput {
                    method: method.to_owned(),
                    reason: "`providerInstanceMutation.instance.driver` must be a string"
                        .to_owned(),
                })?;
            ProviderDriverKind::new(driver).map_err(|err| GuardEvalError::InvalidInput {
                method: method.to_owned(),
                reason: format!("invalid `providerInstanceMutation.instance.driver`: {err}"),
            })?;
            Ok(())
        }
        "remove" => Ok(()),
        other => Err(GuardEvalError::InvalidInput {
            method: method.to_owned(),
            reason: format!("unknown `providerInstanceMutation.operation` `{other}`"),
        }),
    }
}

pub(super) fn eval_server_update_settings(
    input: &Value,
) -> Result<Vec<EnvironmentScope>, GuardEvalError> {
    const METHOD: &str = "server.updateSettings";
    let Some(obj) = input.as_object() else {
        return Err(GuardEvalError::InvalidInput {
            method: METHOD.to_owned(),
            reason: "expected JSON object payload".to_owned(),
        });
    };
    let Some(patch) = obj.get("patch").and_then(Value::as_object) else {
        return Err(GuardEvalError::InvalidInput {
            method: METHOD.to_owned(),
            reason: "missing or non-object `patch` field".to_owned(),
        });
    };
    let mut scopes = required_scopes_for_server_settings_patch(patch)?;
    if let Some(mutation) = obj.get("providerInstanceMutation") {
        validate_provider_instance_mutation(mutation, METHOD)?;
        if patch.is_empty() {
            return Ok(vec![EnvironmentScope::ProvidersManage]);
        }
        if !scopes.contains(&EnvironmentScope::ProvidersManage) {
            scopes.push(EnvironmentScope::ProvidersManage);
        }
    }
    Ok(scopes)
}
