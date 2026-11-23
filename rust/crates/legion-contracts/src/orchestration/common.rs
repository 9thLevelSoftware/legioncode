//! Shared value types and forward-compatible field wrappers for orchestration v2.

use std::collections::HashMap;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use serde_json::Value;

use crate::ids::{
    ContextHandoffId, EnvironmentId, NodeId, PlanId, ProviderDriverKind, ProviderThreadId,
    ProviderTurnId, RunId, ScheduledTaskId, ThreadId,
};

/// Three-state patch value distinguishing omitted fields from explicit JSON `null`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum PatchValue<T> {
    /// Field was omitted on the wire (leave existing value unchanged).
    #[default]
    Absent,
    /// Field was explicitly set to JSON `null` (clear existing value).
    Null,
    /// Field was explicitly set to `T`.
    Set(T),
}

impl<T> PatchValue<T> {
    /// Returns `true` when the patch value is [`PatchValue::Absent`].
    pub fn is_absent(&self) -> bool {
        matches!(self, Self::Absent)
    }
}

impl<T: Serialize> Serialize for PatchValue<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Absent | Self::Null => serializer.serialize_none(),
            Self::Set(value) => value.serialize(serializer),
        }
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for PatchValue<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match Option::<T>::deserialize(deserializer)? {
            None => Ok(Self::Null),
            Some(value) => Ok(Self::Set(value)),
        }
    }
}

pub(crate) fn deserialize_trimmed_non_empty<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = String::deserialize(deserializer)?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(de::Error::custom(
            "string cannot be empty or whitespace-only",
        ));
    }
    Ok(trimmed.to_owned())
}

/// Origin actor of an orchestration action or message (`orchestrationV2.ts:92`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OrchestrationActor {
    /// Interactive human user.
    User,
    /// Autonomous coding agent.
    Agent,
    /// Internal server or scheduler event.
    System,
}

/// Surface or subsystem that created a thread or message (`orchestrationV2.ts:95-101`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CreationSource {
    /// Web/desktop browser UI.
    Web,
    /// Native mobile app.
    Mobile,
    /// Model Context Protocol client.
    Mcp,
    /// Provider-native subagent spawn.
    Provider,
    /// Internal server process.
    Server,
}

/// How a thread's initial history was populated (`orchestrationV2.ts:104`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ThreadHistoryOrigin {
    /// Created natively under orchestration v2.
    #[serde(rename = "native")]
    Native,
    /// Imported from orchestration v1 state.
    #[serde(rename = "v1_import")]
    V1Import,
}

/// Confidence of a provider-native reference (`orchestrationV2.ts:157`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NativeRefStrength {
    /// Strong deterministic ID from the provider.
    Strong,
    /// Heuristic or fingerprinted match.
    Weak,
    /// No native reference available.
    None,
}

/// Provider-native entity identifier and matching metadata (`orchestrationV2.ts:160-166`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRef {
    /// Provider driver kind.
    pub driver: ProviderDriverKind,
    /// Native identifier, or `null`.
    pub native_id: Option<String>,
    /// Confidence level of this reference.
    pub strength: NativeRefStrength,
    /// Optional content fingerprint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    /// Optional ordinal within a multi-turn sequence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ordinal: Option<u64>,
}

/// MCP client access tier granted to a linked environment (`auth.ts:270-276`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum McpClientAccess {
    /// Read-only access.
    ReadOnly,
    /// Supervised mode.
    ApprovalRequired,
    /// Auto-accept edits mode.
    AutoAcceptEdits,
    /// Auto mode.
    Auto,
    /// Unbounded full access.
    FullAccess,
}

/// Linked environment provenance stamped on threads created over MCP (`orchestrationV2.ts:118-122`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkOrigin {
    /// MCP OAuth session ID.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub session_id: String,
    /// Display label of the peer client.
    pub label: String,
    /// Access tier paired with this link.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<McpClientAccess>,
}

/// Parent thread provenance in a linked environment for remote subagent threads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DelegatedFrom {
    /// Source environment identifier.
    pub environment_id: EnvironmentId,
    /// Parent thread identifier on the source environment.
    pub thread_id: ThreadId,
    /// Parent thread title at delegation time.
    pub title: String,
}

/// Cross-environment sender attribution attached to a dispatched message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkedSender {
    /// Source environment identifier.
    pub environment_id: EnvironmentId,
    /// Sending thread identifier.
    pub thread_id: ThreadId,
    /// Sending thread title at send time.
    pub title: String,
}

/// Helper alias for dynamic JSON records such as provider approval answers.
pub type JsonRecord = HashMap<String, Value>;

/// Shared payload references in `message.dispatch` (`orchestrationV2.ts:3439`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourcePlanRef {
    /// Thread containing the source plan.
    pub thread_id: ThreadId,
    /// Plan artifact ID.
    pub plan_id: PlanId,
}

/// Delegated completion batch metadata (`orchestrationV2.ts:3447-3452`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DelegatedCompletionRef {
    /// Parent run identifier.
    pub parent_run_id: RunId,
    /// Monotonic delivery generation.
    pub generation: u64,
    /// Completed subagent task node IDs.
    pub task_ids: Vec<NodeId>,
}

/// Unused helper marker keeping scheduled task ID references imported cleanly.
pub type ScheduledTaskRef = ScheduledTaskId;
/// Unused helper marker keeping context handoff ID references imported cleanly.
pub type ContextHandoffRef = ContextHandoffId;
/// Unused helper marker keeping provider turn/thread ID references imported cleanly.
pub type ProviderRefsPair = (ProviderThreadId, ProviderTurnId);
