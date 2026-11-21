//! Thread aggregate and visit state domain models (`orchestrationV2.ts`).

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::ids::{CommandId, ProjectId, ProviderInstanceId, ProviderThreadId, ThreadId};
use crate::policy::RuntimeMode;
use crate::pull_requests::{ThreadLinkedPullRequest, ThreadPullRequestLink};

use super::command_payloads::ProviderInteractionMode;
use super::common::{CreationSource, DelegatedFrom, LinkOrigin, OrchestrationActor, PatchValue};
use super::model_selection::{LimitRecovery, ModelSelection};

fn deserialize_lenient_tags<'de, D>(deserializer: D) -> Result<Option<Vec<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = Value::deserialize(deserializer)?;
    Ok(Some(
        serde_json::from_value::<Vec<String>>(raw).unwrap_or_default(),
    ))
}

fn deserialize_lenient_opens<'de, D>(deserializer: D) -> Result<PatchValue<Value>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = Value::deserialize(deserializer)?;
    if raw.is_null() {
        return Ok(PatchValue::Null);
    }
    Ok(PatchValue::Set(raw))
}

/// Thread lineage describing root, fork, and subagent hierarchy (`orchestrationV2.ts:169-175`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadLineage {
    /// Immediate parent thread ID, or `null` for root threads.
    pub parent_thread_id: Option<ThreadId>,
    /// Relationship kind (`"fork"` or `"subagent"`), or `null`.
    pub relationship_to_parent: Option<String>,
    /// Ultimate root thread ID in the lineage tree.
    pub root_thread_id: ThreadId,
    /// Whether this thread is a hidden side chat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side_chat: Option<bool>,
}

/// Core thread aggregate persisted and broadcast via domain events (`orchestrationV2.ts:472-550`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppThread {
    /// Actor that created the thread.
    pub created_by: OrchestrationActor,
    /// Surface that created the thread.
    pub creation_source: CreationSource,
    /// Unique thread ID.
    pub id: ThreadId,
    /// Owning project ID.
    pub project_id: ProjectId,
    /// Display title.
    pub title: String,
    /// Active provider instance ID.
    pub provider_instance_id: ProviderInstanceId,
    /// Active model selection.
    pub model_selection: ModelSelection,
    /// Active runtime permission mode.
    pub runtime_mode: RuntimeMode,
    /// Active provider interaction mode.
    pub interaction_mode: ProviderInteractionMode,
    /// Checked-out branch name, or `null`.
    pub branch: Option<String>,
    /// Active git worktree path, or `null`.
    pub worktree_path: Option<String>,
    /// Legacy single linked pull request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked_pull_request: Option<Option<ThreadLinkedPullRequest>>,
    /// Multi-PR links (`Some(vec![])` stays distinct from `None`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull_requests: Option<Vec<ThreadPullRequestLink>>,
    /// Pull request associated with the current branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_pull_request: Option<Option<ThreadLinkedPullRequest>>,
    /// Linked issues.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issues: Option<Vec<Value>>,
    /// Currently attached provider thread ID, or `null`.
    pub active_provider_thread_id: Option<ProviderThreadId>,
    /// Origin (`native` or `v1_import`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_origin: Option<String>,
    /// MCP link provenance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_origin: Option<LinkOrigin>,
    /// Remote parent thread provenance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delegated_from: Option<DelegatedFrom>,
    /// Lineage tree coordinates.
    pub lineage: ThreadLineage,
    /// Fork source point, or `null`.
    pub forked_from: Option<Value>,
    /// Creation timestamp.
    pub created_at: String,
    /// Last update timestamp.
    pub updated_at: String,
    /// Archive timestamp, or `null`.
    pub archived_at: Option<String>,
    /// Settlement override (`settled` or `active`), defaulting to `null`.
    #[serde(default)]
    pub settled_override: Option<String>,
    /// Settlement timestamp, defaulting to `null`.
    #[serde(default)]
    pub settled_at: Option<String>,
    /// Unsettled timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unsettled_at: Option<Option<String>>,
    /// Snooze expiration timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snoozed_until: Option<Option<String>>,
    /// Snooze start timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snoozed_at: Option<Option<String>>,
    /// Last snooze wake timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_snooze_wake_at: Option<Option<String>>,
    /// Usage-limit recovery configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit_recovery: Option<Option<LimitRecovery>>,
    /// Active guest share state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guest_share: Option<Option<Value>>,
    /// Pin timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned_at: Option<Option<String>>,
    /// Auto-settle opt-out timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_settle_disabled_at: Option<Option<String>>,
    /// Pinned sort order key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin_order_key: Option<Option<String>>,
    /// Active sort order key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_order_key: Option<Option<String>>,
    /// Thread tags (leniently decoded as empty on invalid entries).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_lenient_tags"
    )]
    pub tags: Option<Vec<String>>,
    /// Last visited timestamp, defaulting to `null`.
    #[serde(default)]
    pub last_visited_at: Option<String>,
    /// Thread open ranking stats (leniently decoded).
    #[serde(
        default,
        skip_serializing_if = "PatchValue::is_absent",
        deserialize_with = "deserialize_lenient_opens"
    )]
    pub opens: PatchValue<Value>,
    /// In-flight title regeneration state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_regeneration: Option<Option<Value>>,
    /// Last completed title regeneration state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_title_regeneration: Option<Option<Value>>,
    /// Whether the user manually renamed the thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_renamed: Option<bool>,
    /// Pending rollback command ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollback_request_id: Option<CommandId>,
    /// Last rollback failure summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollback_failure: Option<Option<Value>>,
    /// Soft-deletion timestamp, or `null`.
    pub deleted_at: Option<String>,
}

/// Lightweight visit update payload (`orchestrationV2.ts:554-557`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadVisit {
    /// Updated read watermark, or `null`.
    #[serde(default)]
    pub last_visited_at: Option<String>,
    /// Updated open ranking stats.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opens: Option<Option<Value>>,
}
