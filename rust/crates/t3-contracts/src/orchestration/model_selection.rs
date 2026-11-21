//! Model selection, workspace launch strategies, recovery, failure, and context types.

use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::Value;

use crate::ids::{CheckpointId, CommandId, ProviderInstanceId, RunId, ThreadId, TurnItemId};

use super::common::ProviderRef;

/// Single option key/value selection on a model (`model.ts:51-54`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderOptionSelection {
    /// Option identifier.
    pub id: String,
    /// Selected value (`string` or `boolean`).
    pub value: ProviderOptionValue,
}

/// Value of a provider model option (`model.ts:48`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ProviderOptionValue {
    /// String option value.
    Text(String),
    /// Boolean toggle option value.
    Bool(bool),
}

/// Provider instance + model + option selection (`modelSelection.ts`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSelection {
    /// Target provider instance identifier.
    pub instance_id: ProviderInstanceId,
    /// Selected model identifier.
    pub model: String,
    /// Optional provider-specific model options.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<ProviderOptionSelection>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawModelSelection {
    #[serde(default)]
    provider: Option<Value>,
    #[serde(default)]
    instance_id: Option<Value>,
    model: Value,
    #[serde(default)]
    options: Option<Value>,
}

impl<'de> Deserialize<'de> for ModelSelection {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = RawModelSelection::deserialize(deserializer)?;
        let instance_val = raw
            .instance_id
            .or_else(|| raw.provider.filter(Value::is_string))
            .ok_or_else(|| de::Error::missing_field("instanceId"))?;
        let instance_id =
            ProviderInstanceId::deserialize(instance_val).map_err(de::Error::custom)?;
        let model_raw = String::deserialize(raw.model).map_err(de::Error::custom)?;
        let model = model_raw.trim().to_owned();
        if model.is_empty() {
            return Err(de::Error::custom("model cannot be empty"));
        }
        let options = match raw.options {
            None | Some(Value::Null) => None,
            Some(Value::Array(items)) => Some(
                items
                    .into_iter()
                    .map(ProviderOptionSelection::deserialize)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(de::Error::custom)?,
            ),
            Some(Value::Object(map)) => Some(
                map.into_iter()
                    .filter_map(|(k, v)| {
                        let id = k.trim().to_owned();
                        if id.is_empty() {
                            return None;
                        }
                        match v {
                            Value::String(s) if !s.trim().is_empty() => {
                                Some(ProviderOptionSelection {
                                    id,
                                    value: ProviderOptionValue::Text(s.trim().to_owned()),
                                })
                            }
                            Value::Bool(b) => Some(ProviderOptionSelection {
                                id,
                                value: ProviderOptionValue::Bool(b),
                            }),
                            _ => None,
                        }
                    })
                    .collect(),
            ),
            Some(_) => return Err(de::Error::custom("invalid modelSelection.options")),
        };
        Ok(Self {
            instance_id,
            model,
            options,
        })
    }
}

/// Workspace launch strategy when starting a thread (`orchestrationV2.ts:744-760`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkspaceStrategy {
    /// Use the repository root directly.
    Root {
        /// Optional branch to check out.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        branch: Option<String>,
    },
    /// Attach to an existing git worktree path.
    #[serde(rename_all = "camelCase")]
    ExistingWorktree {
        /// Filesystem path of the existing worktree.
        worktree_path: String,
        /// Optional branch name.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        branch: Option<String>,
    },
    /// Create a fresh git worktree from `base_ref`.
    #[serde(rename_all = "camelCase")]
    Worktree {
        /// Base git ref or branch name.
        base_ref: String,
        /// Optional new branch name.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        branch: Option<String>,
        /// Whether to branch from origin instead of local HEAD.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        start_from_origin: Option<bool>,
    },
}

/// Usage-limit recovery policy state (`orchestrationV2.ts:422-443`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LimitRecovery {
    /// Optional command ID that requested recovery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<CommandId>,
    /// Limited run identifier.
    pub run_id: RunId,
    /// ISO timestamp when the quota resets.
    pub reset_at: String,
    /// Automatically resume when quota resets.
    pub auto_resume: bool,
    /// Optional snooze toggle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snooze: Option<bool>,
}

/// Partial update to usage-limit recovery settings (`orchestrationV2.ts:431-443`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LimitRecoveryUpdate {
    /// Limited run identifier.
    pub run_id: RunId,
    /// ISO timestamp when the quota resets.
    pub reset_at: String,
    /// Optional auto-resume toggle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_resume: Option<bool>,
    /// Optional snooze toggle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snooze: Option<bool>,
}

/// Provider failure classification (`orchestrationV2.ts:1595-1617`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderFailureClass {
    /// Quota or rate limit reached.
    UsageLimit,
    /// Provider API or runtime error.
    ProviderError,
    /// Network or IPC transport failure.
    TransportError,
    /// Authorization or filesystem permission failure.
    PermissionError,
    /// Request or payload validation failure.
    ValidationError,
    /// Unclassified error.
    Unknown,
}

/// Redacted provider failure summary (`orchestrationV2.ts:1611-1617`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderFailure {
    /// Failure category.
    pub class: ProviderFailureClass,
    /// Human-readable error message.
    pub message: String,
    /// Optional provider error code.
    pub code: Option<String>,
    /// Whether retrying may succeed.
    pub retryable: Option<bool>,
    /// Optional quota reset timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset_at: Option<Option<String>>,
}

/// Retry progress metadata on an error turn item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRetry {
    /// 1-based attempt number.
    pub attempt: u64,
    /// Configured maximum attempts, or `null`.
    pub max_attempts: Option<u64>,
    /// Delay before retry in milliseconds, or `null`.
    pub retry_delay_ms: Option<u64>,
}

/// Background-work wake notification attached to a message (`orchestrationV2.ts:1378-1425`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestrationNotification {
    /// Source descriptor (kept flexible for `kindUnionWithFallback` legacy variants).
    pub source: Value,
    /// Notification outcome (`completed`, `failed`, `cancelled`, `updated`, `unknown`).
    pub outcome: String,
    /// Short human-readable summary.
    pub summary: String,
    /// Optional detailed message body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Source point for context handoffs and thread forks (`orchestrationV2.ts:190-204`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ThreadForkSourcePoint {
    /// Fork from the latest stable checkpoint/run.
    LatestStable,
    /// Fork from a specific run.
    #[serde(rename_all = "camelCase")]
    Run {
        /// Source run identifier.
        run_id: RunId,
    },
    /// Fork from a specific checkpoint.
    #[serde(rename_all = "camelCase")]
    Checkpoint {
        /// Source checkpoint identifier.
        checkpoint_id: CheckpointId,
    },
}

/// Context handoff/transfer provenance point (`orchestrationV2.ts:190-197`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextSourcePoint {
    /// Source thread ID.
    pub thread_id: ThreadId,
    /// Optional run ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    /// Optional checkpoint ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint_id: Option<CheckpointId>,
    /// Optional turn item ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_item_id: Option<TurnItemId>,
    /// Optional provider thread reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_thread_ref: Option<ProviderRef>,
    /// Optional provider turn reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_turn_ref: Option<ProviderRef>,
}
