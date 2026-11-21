//! Typed ACP `session/update` notifications and forward-compatible parsing.

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Value, json};

use super::presentation::ToolCallUpdatePayload;
use super::protocol::PromptContentBlock;

/// Slash command descriptor announced via `available_commands_update`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvailableCommand {
    /// Command trigger name without leading slash.
    pub name: String,
    /// Concise user-facing description.
    #[serde(default)]
    pub description: String,
    /// Optional argument hint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<Value>,
}

/// Typed ACP `session/update` payload variants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionUpdateKind {
    /// Assistant output text/content delta.
    AgentMessageChunk(PromptContentBlock),
    /// Reasoning/thinking delta.
    AgentThoughtChunk(PromptContentBlock),
    /// User message echo delta.
    UserMessageChunk(PromptContentBlock),
    /// Initial tool-call creation notification.
    ToolCall(ToolCallUpdatePayload),
    /// Incremental tool-call progress/completion notification.
    ToolCallUpdate(ToolCallUpdatePayload),
    /// Plan entries update.
    Plan {
        /// Ordered plan entry objects.
        entries: Vec<Value>,
    },
    /// Advertised slash commands update.
    AvailableCommandsUpdate {
        /// Updated slash commands.
        available_commands: Vec<AvailableCommand>,
    },
    /// Current session interaction mode update.
    CurrentModeUpdate {
        /// Newly active mode ID.
        current_mode_id: String,
    },
    /// Session config option state update.
    ConfigOptionUpdate {
        /// Updated option descriptors.
        config_options: Vec<Value>,
    },
    /// Session title/metadata update.
    SessionInfoUpdate {
        /// Updated session title if supplied.
        title: Option<String>,
        /// Timestamp string if supplied.
        updated_at: Option<String>,
    },
    /// Context window/cost telemetry update.
    UsageUpdate {
        /// Tokens currently consumed.
        used: Option<u64>,
        /// Maximum context window size.
        size: Option<u64>,
        /// Optional cost descriptor.
        cost: Option<Value>,
    },
    /// Forward-compatible unknown update kind.
    Unknown {
        /// Unrecognized `"sessionUpdate"` tag.
        session_update: String,
        /// Full raw update JSON object.
        raw: Value,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChunkEnvelope {
    content: PromptContentBlock,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlanEnvelope {
    #[serde(default)]
    entries: Vec<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommandsEnvelope {
    #[serde(default)]
    available_commands: Vec<AvailableCommand>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModeEnvelope {
    current_mode_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfigOptionsEnvelope {
    #[serde(default)]
    config_options: Vec<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionInfoEnvelope {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageEnvelope {
    #[serde(default)]
    used: Option<u64>,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    cost: Option<Value>,
}

impl<'de> Deserialize<'de> for SessionUpdateKind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = Value::deserialize(deserializer)?;
        let Some(tag) = raw.get("sessionUpdate").and_then(Value::as_str) else {
            return Err(D::Error::missing_field("sessionUpdate"));
        };
        let tag_owned = tag.to_owned();
        match tag {
            "agent_message_chunk" => serde_json::from_value::<ChunkEnvelope>(raw)
                .map(|c| Self::AgentMessageChunk(c.content))
                .map_err(D::Error::custom),
            "agent_thought_chunk" => serde_json::from_value::<ChunkEnvelope>(raw)
                .map(|c| Self::AgentThoughtChunk(c.content))
                .map_err(D::Error::custom),
            "user_message_chunk" => serde_json::from_value::<ChunkEnvelope>(raw)
                .map(|c| Self::UserMessageChunk(c.content))
                .map_err(D::Error::custom),
            "tool_call" => serde_json::from_value(raw)
                .map(Self::ToolCall)
                .map_err(D::Error::custom),
            "tool_call_update" => serde_json::from_value(raw)
                .map(Self::ToolCallUpdate)
                .map_err(D::Error::custom),
            "plan" => serde_json::from_value::<PlanEnvelope>(raw)
                .map(|p| Self::Plan { entries: p.entries })
                .map_err(D::Error::custom),
            "available_commands_update" => serde_json::from_value::<CommandsEnvelope>(raw)
                .map(|c| Self::AvailableCommandsUpdate {
                    available_commands: c.available_commands,
                })
                .map_err(D::Error::custom),
            "current_mode_update" => serde_json::from_value::<ModeEnvelope>(raw)
                .map(|m| Self::CurrentModeUpdate {
                    current_mode_id: m.current_mode_id,
                })
                .map_err(D::Error::custom),
            "config_option_update" => serde_json::from_value::<ConfigOptionsEnvelope>(raw)
                .map(|c| Self::ConfigOptionUpdate {
                    config_options: c.config_options,
                })
                .map_err(D::Error::custom),
            "session_info_update" => serde_json::from_value::<SessionInfoEnvelope>(raw)
                .map(|s| Self::SessionInfoUpdate {
                    title: s.title,
                    updated_at: s.updated_at,
                })
                .map_err(D::Error::custom),
            "usage_update" => serde_json::from_value::<UsageEnvelope>(raw)
                .map(|u| Self::UsageUpdate {
                    used: u.used,
                    size: u.size,
                    cost: u.cost,
                })
                .map_err(D::Error::custom),
            _ => Ok(Self::Unknown {
                session_update: tag_owned,
                raw,
            }),
        }
    }
}

/// Notification parameters for ACP `session/update`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionUpdateNotification {
    /// Target ACP session ID.
    pub session_id: String,
    /// Decoded streaming update variant.
    pub update: SessionUpdateKind,
}

impl Serialize for SessionUpdateKind {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let value = match self {
            Self::AgentMessageChunk(content) => {
                json!({ "sessionUpdate": "agent_message_chunk", "content": content })
            }
            Self::AgentThoughtChunk(content) => {
                json!({ "sessionUpdate": "agent_thought_chunk", "content": content })
            }
            Self::UserMessageChunk(content) => {
                json!({ "sessionUpdate": "user_message_chunk", "content": content })
            }
            Self::ToolCall(payload) => serialize_tagged_payload("tool_call", payload),
            Self::ToolCallUpdate(payload) => serialize_tagged_payload("tool_call_update", payload),
            Self::Plan { entries } => json!({ "sessionUpdate": "plan", "entries": entries }),
            Self::AvailableCommandsUpdate { available_commands } => json!({
                "sessionUpdate": "available_commands_update",
                "availableCommands": available_commands,
            }),
            Self::CurrentModeUpdate { current_mode_id } => json!({
                "sessionUpdate": "current_mode_update",
                "currentModeId": current_mode_id,
            }),
            Self::ConfigOptionUpdate { config_options } => json!({
                "sessionUpdate": "config_option_update",
                "configOptions": config_options,
            }),
            Self::SessionInfoUpdate { title, updated_at } => json!({
                "sessionUpdate": "session_info_update",
                "title": title,
                "updatedAt": updated_at,
            }),
            Self::UsageUpdate { used, size, cost } => json!({
                "sessionUpdate": "usage_update",
                "used": used,
                "size": size,
                "cost": cost,
            }),
            Self::Unknown { raw, .. } => raw.clone(),
        };
        value.serialize(serializer)
    }
}

fn serialize_tagged_payload(tag: &str, payload: &ToolCallUpdatePayload) -> Value {
    let mut val = serde_json::to_value(payload).unwrap_or_else(|_| json!({}));
    if let Some(obj) = val.as_object_mut() {
        obj.insert("sessionUpdate".to_owned(), Value::String(tag.to_owned()));
    }
    val
}
