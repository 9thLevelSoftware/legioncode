//! Lossless wire command decoders for V2 client commands, project mutations, and legacy aliases.

use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::ids::{CommandId, ProjectId};

/// Default thread workspace environment mode (`packages/contracts/src/environment.ts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThreadEnvMode {
    /// Reuse the project's primary workspace directory.
    Local,
    /// Isolate work in a dedicated git worktree.
    Worktree,
}

use super::command_meta::is_internal_command_type;
use super::commands::OrchestrationCommand;
use super::common::PatchValue;
use super::model_selection::ModelSelection;

fn deserialize_present_optional<'de, T, D>(deserializer: D) -> Result<Option<T>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Option::<T>::deserialize(deserializer)
}

/// Wire payload for `project.create` (`packages/contracts/src/project.ts`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectCreateWireCommand {
    /// Idempotency command ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_id: Option<CommandId>,
    /// Optional explicit project ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<ProjectId>,
    /// Project display title.
    pub title: String,
    /// Absolute workspace root directory path.
    pub workspace_root: String,
    /// Optional flag to create missing directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub create_workspace_root_if_missing: Option<bool>,
    /// Optional default model selection.
    #[serde(default, skip_serializing_if = "PatchValue::is_absent")]
    pub default_model_selection: PatchValue<ModelSelection>,
    /// Configured scripts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scripts: Option<Vec<Value>>,
}

/// Wire payload for `project.meta.update` / `project.update`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectUpdateWireCommand {
    /// Idempotency command ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_id: Option<CommandId>,
    /// Target project ID.
    pub project_id: ProjectId,
    /// Optional updated title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Optional updated workspace root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_root: Option<String>,
    /// Optional default model selection patch.
    #[serde(default, skip_serializing_if = "PatchValue::is_absent")]
    pub default_model_selection: PatchValue<ModelSelection>,
    /// Optional default thread environment mode patch.
    #[serde(default, skip_serializing_if = "PatchValue::is_absent")]
    pub default_thread_env_mode: PatchValue<ThreadEnvMode>,
    /// Optional auto-pull toggle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_pull: Option<bool>,
    /// Optional favicon path patch.
    #[serde(default, skip_serializing_if = "PatchValue::is_absent")]
    pub favicon_path: PatchValue<String>,
    /// Optional project icon patch.
    #[serde(default, skip_serializing_if = "PatchValue::is_absent")]
    pub project_icon: PatchValue<Value>,
    /// Updated scripts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scripts: Option<Vec<Value>>,
    /// Pin toggle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned: Option<bool>,
    /// Archive toggle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
}

/// Client-dispatchable project command variants (`project.ts`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ProjectWireCommand {
    /// Create a workspace project.
    #[serde(rename = "project.create")]
    Create(ProjectCreateWireCommand),
    /// Update project metadata (`project.meta.update` or `project.update`).
    #[serde(rename = "project.meta.update", alias = "project.update")]
    Update(ProjectUpdateWireCommand),
    /// Archive a workspace project.
    #[serde(rename = "project.archive", rename_all = "camelCase")]
    Archive {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command_id: Option<CommandId>,
        project_id: ProjectId,
    },
    /// Unarchive a workspace project.
    #[serde(rename = "project.unarchive", rename_all = "camelCase")]
    Unarchive {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command_id: Option<CommandId>,
        project_id: ProjectId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    /// Soft-delete a workspace project.
    #[serde(rename = "project.delete", rename_all = "camelCase")]
    Delete {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command_id: Option<CommandId>,
        project_id: ProjectId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        force: Option<bool>,
        #[serde(
            default,
            deserialize_with = "deserialize_present_optional",
            skip_serializing_if = "Option::is_none"
        )]
        expected_empty: Option<bool>,
    },
}

/// Union of all client wire commands (orchestration thread commands and project mutations).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireClientCommand {
    /// Any of the 58 V2 thread orchestration commands (or supported legacy V1 aliases).
    Orchestration(Box<OrchestrationCommand>),
    /// Project creation, update, archive, unarchive, or delete command.
    Project(Box<ProjectWireCommand>),
}

/// Decodes a wire JSON command payload into [`OrchestrationCommand`].
///
/// Accepts all 58 canonical `OrchestrationV2Command` wire shapes and losslessly translates
/// legacy V1 aliases (`thread.turn.start`, `thread.turn.interrupt`, `thread.approval.respond`,
/// `thread.user-input.respond`, `thread.checkpoint.revert`), while strictly rejecting
/// `INTERNAL_COMMAND_TYPES`.
pub fn decode_wire_orchestration_command(
    raw: Value,
) -> Result<OrchestrationCommand, serde_json::Error> {
    let Some(obj) = raw.as_object() else {
        return serde_json::from_value(raw);
    };
    let Some(cmd_type) = obj.get("type").and_then(Value::as_str) else {
        return serde_json::from_value(raw);
    };
    if is_internal_command_type(cmd_type) {
        return Err(de::Error::custom(format!(
            "internal server command `{cmd_type}` is rejected on client dispatch"
        )));
    }
    let normalized = match cmd_type {
        "thread.turn.start" => normalize_turn_start(obj)?,
        "thread.turn.interrupt" => normalize_turn_interrupt(obj),
        "thread.approval.respond" | "thread.user-input.respond" => {
            normalize_runtime_request_respond(obj)
        }
        "thread.checkpoint.revert" => normalize_checkpoint_revert(obj),
        _ => raw,
    };
    serde_json::from_value(normalized)
}

/// Decodes any client wire command (`OrchestrationCommand` or `ProjectWireCommand`).
pub fn decode_wire_client_command(raw: Value) -> Result<WireClientCommand, serde_json::Error> {
    let cmd_type = raw
        .as_object()
        .and_then(|obj| obj.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if matches!(
        cmd_type,
        "project.create"
            | "project.meta.update"
            | "project.update"
            | "project.archive"
            | "project.unarchive"
            | "project.delete"
    ) {
        let proj: ProjectWireCommand = serde_json::from_value(raw)?;
        return Ok(WireClientCommand::Project(Box::new(proj)));
    }
    let orch = decode_wire_orchestration_command(raw)?;
    Ok(WireClientCommand::Orchestration(Box::new(orch)))
}

fn normalize_turn_start(obj: &Map<String, Value>) -> Result<Value, serde_json::Error> {
    let mut out = obj.clone();
    out.insert(
        "type".to_owned(),
        Value::String("message.dispatch".to_owned()),
    );
    if let Some(msg) = obj.get("message").and_then(Value::as_object) {
        if !out.contains_key("messageId")
            && let Some(id) = msg.get("messageId")
        {
            out.insert("messageId".to_owned(), id.clone());
        }
        if !out.contains_key("text")
            && let Some(text) = msg.get("text")
        {
            out.insert("text".to_owned(), text.clone());
        }
        if !out.contains_key("attachments")
            && let Some(attachments) = msg.get("attachments")
        {
            out.insert("attachments".to_owned(), attachments.clone());
        }
        if !out.contains_key("context")
            && let Some(context) = msg.get("context")
        {
            out.insert("context".to_owned(), context.clone());
        }
    }
    out.entry("createdBy")
        .or_insert_with(|| Value::String("user".to_owned()));
    out.entry("creationSource")
        .or_insert_with(|| Value::String("web".to_owned()));
    out.entry("attachments")
        .or_insert_with(|| Value::Array(Vec::new()));
    out.entry("text")
        .or_insert_with(|| Value::String(String::new()));
    out.entry("dispatchMode")
        .or_insert_with(|| json!({ "type": "start_immediately" }));
    if !out.contains_key("messageId") {
        let fallback_id = out
            .get("commandId")
            .and_then(Value::as_str)
            .map(|c| format!("msg-{c}"))
            .ok_or_else(|| de::Error::missing_field("messageId"))?;
        out.insert("messageId".to_owned(), Value::String(fallback_id));
    }
    Ok(Value::Object(out))
}

fn normalize_turn_interrupt(obj: &Map<String, Value>) -> Value {
    let mut out = obj.clone();
    if let Some(turn_id) = obj.get("turnId").filter(|v| !v.is_null())
        && !out.contains_key("runId")
    {
        out.insert("runId".to_owned(), turn_id.clone());
    }
    if out.get("runId").is_some_and(|v| !v.is_null()) {
        out.insert("type".to_owned(), Value::String("run.interrupt".to_owned()));
    } else {
        out.remove("runId");
        out.insert("type".to_owned(), Value::String("thread.stop".to_owned()));
    }
    Value::Object(out)
}

fn normalize_runtime_request_respond(obj: &Map<String, Value>) -> Value {
    let mut out = obj.clone();
    out.insert(
        "type".to_owned(),
        Value::String("runtime-request.respond".to_owned()),
    );
    Value::Object(out)
}

fn normalize_checkpoint_revert(obj: &Map<String, Value>) -> Value {
    let mut out = obj.clone();
    out.insert(
        "type".to_owned(),
        Value::String("checkpoint.rollback".to_owned()),
    );
    if !out.contains_key("scopeId") {
        let scope = obj
            .get("threadId")
            .and_then(Value::as_str)
            .map_or_else(|| "scope-default".to_owned(), |tid| format!("scope-{tid}"));
        out.insert("scopeId".to_owned(), Value::String(scope));
    }
    if !out.contains_key("checkpointId") {
        let cp = out
            .get("turnCount")
            .and_then(Value::as_u64)
            .map_or_else(|| "turn-0".to_owned(), |n| format!("turn-{n}"));
        out.insert("checkpointId".to_owned(), Value::String(cp));
    }
    Value::Object(out)
}

/// Returns the default structured `OrchestrationV2ProviderCapabilities` JSON object.
pub fn default_wire_provider_capabilities() -> Value {
    serde_json::from_str(include_str!("default_wire_provider_capabilities.json"))
        .unwrap_or(Value::Null)
}

/// Computes raw 32-byte SHA-256 digest of `bytes`.
pub fn sha256_digest(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().into()
}
