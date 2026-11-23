//! Turn items, projected timeline items, and strict `type`-tag decoding (fixing BUG-18).

use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::Value;

use crate::ids::{NodeId, ProviderThreadId, ProviderTurnId, RunId, ThreadId, TurnItemId};

use super::common::ProviderRef;
use super::item_kinds::TurnItemKind;

/// Turn item lifecycle status (`orchestrationV2.ts:1573-1584`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnItemStatus {
    /// Not started.
    Idle,
    /// Queued or awaiting start.
    Pending,
    /// Currently executing.
    Running,
    /// Paused awaiting user or background input.
    Waiting,
    /// Finished successfully.
    Completed,
    /// Failed with an error.
    Failed,
    /// Cancelled before completion.
    Cancelled,
    /// Interrupted mid-turn.
    Interrupted,
}

/// Optional collapsed-work cursor attached on wire projections (`orchestrationV2.ts:1650-1656`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeferredWorkCursor {
    /// Opaque pagination cursor.
    pub cursor: String,
    /// Number of collapsed items.
    pub item_count: u64,
    /// Highest ordinal among collapsed items.
    pub last_ordinal: u64,
}

/// Single step in a `todo_list` item (`orchestrationV2.ts:1467-1474`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanStep {
    /// Step identifier.
    pub id: String,
    /// Step description text.
    pub text: String,
    /// Step state (`pending`, `running`, `completed`).
    pub status: String,
    /// Optional running duration anchor timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_anchor_at: Option<String>,
    /// Optional elapsed duration in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

/// Checkpoint file diff summary (`orchestrationV2.ts:1519-1534`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointFileSummary {
    /// Repository-relative file path.
    pub path: String,
    /// Change kind (`added`, `modified`, `deleted`, etc.).
    pub kind: String,
    /// Line additions.
    pub additions: u64,
    /// Line deletions.
    pub deletions: u64,
}

/// Aggregate file counts when checkpoint file lists are truncated on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointFileTotals {
    /// Total changed files.
    pub count: u64,
    /// Total added lines.
    pub additions: u64,
    /// Total deleted lines.
    pub deletions: u64,
}

/// All 26 known `OrchestrationV2TurnItem` `type` discriminants.
pub const KNOWN_TURN_ITEM_TYPES: &[&str] = &[
    "notification",
    "user_message",
    "assistant_message",
    "reasoning",
    "proposed_plan",
    "todo_list",
    "user_input_request",
    "file_change",
    "command_execution",
    "file_search",
    "web_search",
    "approval_request",
    "checkpoint",
    "run_interrupt_request",
    "run_interrupt_result",
    "system_notice",
    "error",
    "compaction",
    "handoff",
    "fork",
    "thread_created",
    "secret_request",
    "link_request",
    "feedback_request",
    "subagent",
    "dynamic_tool",
];

/// Returns `true` if `item_type` is one of the 26 known turn-item discriminants.
pub fn is_known_turn_item_type(item_type: &str) -> bool {
    KNOWN_TURN_ITEM_TYPES.contains(&item_type)
}

/// Common fields shared by every [`TurnItem`] (`orchestrationV2.ts:1648-1675`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnItemBase {
    /// Collapsed-work cursor, present only on bounded wire responses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deferred_work: Option<DeferredWorkCursor>,
    /// Optional non-execution tool classification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_non_execution_kind: Option<String>,
    /// Tool surface (`browser` or `computer`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_surface: Option<String>,
    /// Tool icon descriptor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_icon: Option<Value>,
    /// Tool attribution source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_source: Option<Value>,
    /// Unique turn item ID.
    pub id: TurnItemId,
    /// Owning thread ID.
    pub thread_id: ThreadId,
    /// Associated run ID, or `null`.
    pub run_id: Option<RunId>,
    /// Associated execution node ID, or `null`.
    pub node_id: Option<NodeId>,
    /// Provider thread ID, or `null`.
    pub provider_thread_id: Option<ProviderThreadId>,
    /// Provider turn ID, or `null`.
    pub provider_turn_id: Option<ProviderTurnId>,
    /// Provider-native item reference, or `null`.
    pub native_item_ref: Option<ProviderRef>,
    /// Parent turn item ID, or `null`.
    pub parent_item_id: Option<TurnItemId>,
    /// Deterministic ordering key within the thread.
    pub ordinal: u64,
    /// Lifecycle status of the item.
    pub status: TurnItemStatus,
    /// Optional display title, or `null`.
    pub title: Option<String>,
    /// Start timestamp, or `null`.
    pub started_at: Option<String>,
    /// Completion timestamp, or `null`.
    pub completed_at: Option<String>,
    /// Last update timestamp.
    pub updated_at: String,
}

/// A typed turn timeline item (`OrchestrationV2TurnItem`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TurnItem {
    /// Common turn-item header fields.
    #[serde(flatten)]
    pub base: TurnItemBase,
    /// Kind-specific turn-item fields and `"type"` tag.
    #[serde(flatten)]
    pub kind: TurnItemKind,
}

/// Outcome of decoding a raw JSON turn-item value while enforcing BUG-18.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TurnItemDecodeError {
    /// `"type"` was a valid string not recognized by this crate version.
    UnknownType(String),
    /// `"type"` was missing/non-string (BUG-18) or a known variant failed validation.
    Invalid(String),
}

pub(crate) fn decode_turn_item_value(value: &Value) -> Result<TurnItem, TurnItemDecodeError> {
    let Some(obj) = value.as_object() else {
        return Err(TurnItemDecodeError::Invalid(
            "expected JSON object for turn item".to_owned(),
        ));
    };
    let Some(Value::String(tag)) = obj.get("type") else {
        return Err(TurnItemDecodeError::Invalid(
            "turn item `type` tag must be a string".to_owned(),
        ));
    };
    if !is_known_turn_item_type(tag) {
        return Err(TurnItemDecodeError::UnknownType(tag.clone()));
    }
    let base: TurnItemBase = serde_json::from_value(value.clone())
        .map_err(|err| TurnItemDecodeError::Invalid(err.to_string()))?;
    let kind: TurnItemKind = serde_json::from_value(value.clone())
        .map_err(|err| TurnItemDecodeError::Invalid(err.to_string()))?;
    Ok(TurnItem { base, kind })
}

impl<'de> Deserialize<'de> for TurnItem {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        decode_turn_item_value(&value).map_err(|err| match err {
            TurnItemDecodeError::UnknownType(tag) => {
                de::Error::custom(format!("unknown turn item type `{tag}`"))
            }
            TurnItemDecodeError::Invalid(msg) => de::Error::custom(msg),
        })
    }
}

/// Visibility classification of a projected turn item (`orchestrationV2.ts:2007`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TurnItemVisibility {
    /// Originated directly on this thread.
    Local,
    /// Inherited from a fork ancestor thread.
    Inherited,
    /// Synthesized by projection rules.
    Synthetic,
}

/// Projected timeline entry wrapping a [`TurnItem`] (`orchestrationV2.ts:2005-2012`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectedTurnItem {
    /// Zero-based position in the projected timeline.
    pub position: u64,
    /// Timeline visibility category.
    pub visibility: TurnItemVisibility,
    /// Thread that owns the underlying item.
    pub source_thread_id: ThreadId,
    /// Underlying item ID.
    pub source_item_id: TurnItemId,
    /// The turn item payload.
    pub item: TurnItem,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawProjectedTurnItem {
    position: u64,
    visibility: TurnItemVisibility,
    source_thread_id: ThreadId,
    source_item_id: TurnItemId,
    item: Value,
}

pub(crate) fn deserialize_forward_compatible_turn_items<'de, D>(
    deserializer: D,
) -> Result<Vec<TurnItem>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw_items = Vec::<Value>::deserialize(deserializer)?;
    let mut items = Vec::with_capacity(raw_items.len());
    for raw in raw_items {
        match decode_turn_item_value(&raw) {
            Ok(item) => items.push(item),
            Err(TurnItemDecodeError::UnknownType(_)) => {}
            Err(TurnItemDecodeError::Invalid(msg)) => return Err(de::Error::custom(msg)),
        }
    }
    Ok(items)
}

pub(crate) fn deserialize_forward_compatible_projected_items<'de, D>(
    deserializer: D,
) -> Result<Vec<ProjectedTurnItem>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw_rows = Vec::<RawProjectedTurnItem>::deserialize(deserializer)?;
    let mut rows = Vec::with_capacity(raw_rows.len());
    for raw in raw_rows {
        match decode_turn_item_value(&raw.item) {
            Ok(item) => rows.push(ProjectedTurnItem {
                position: raw.position,
                visibility: raw.visibility,
                source_thread_id: raw.source_thread_id,
                source_item_id: raw.source_item_id,
                item,
            }),
            Err(TurnItemDecodeError::UnknownType(_)) => {}
            Err(TurnItemDecodeError::Invalid(msg)) => return Err(de::Error::custom(msg)),
        }
    }
    Ok(rows)
}
