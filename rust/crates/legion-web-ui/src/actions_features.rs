//! Structured payload types for Wave 5 `WebAppAction` variants, keeping `actions.rs` <= 320 LOC.

use legion_client_core::linked_threads::LinkedThreadTarget;
use legion_contracts::mcp_sign_in::{McpSignInStatus, OrchestrationMcpSignInNotice};
use legion_contracts::orchestration::SecretRequestField;
use legion_contracts::peer_links::PeerLinkFailureKind;
use legion_contracts::workspace_folders::WorkspaceScannedFolder;
use serde::{Deserialize, Serialize};

use crate::state_features::{
    FleetEnvironmentEdge, FleetEnvironmentNode, KeybindingEntry, LinkedPaneMessage,
    LinkedPaneSurfaceMode, McpSignInUiStatus, ThreadErrorSource, UpdateCapsulePhase,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordThreadErrorPayload {
    pub thread_id: String,
    pub message: String,
    pub source: ThreadErrorSource,
    #[serde(default)]
    pub runtime_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DismissThreadErrorPayload {
    pub thread_id: String,
    #[serde(default)]
    pub occurrence_id: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetEnvironmentsFleetPayload {
    pub nodes: Vec<FleetEnvironmentNode>,
    #[serde(default)]
    pub edges: Vec<FleetEnvironmentEdge>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdvanceBatchLinkPayload {
    pub environment_id: String,
    pub succeeded: bool,
    #[serde(default)]
    pub failure_kind: Option<PeerLinkFailureKind>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenLinkedThreadPanePayload {
    pub target: LinkedThreadTarget,
    pub title: String,
    pub mode: LinkedPaneSurfaceMode,
    #[serde(default)]
    pub working: bool,
    #[serde(default)]
    pub messages: Vec<LinkedPaneMessage>,
    #[serde(default)]
    pub has_earlier: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetProjectRepositoriesPayload {
    pub project_id: String,
    pub tracked_folders: Vec<String>,
    #[serde(default)]
    pub scanned_folders: Vec<WorkspaceScannedFolder>,
    #[serde(default)]
    pub default_worktree_repos: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRepoFolderPayload {
    pub project_id: String,
    pub folder_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadRepoPinPayload {
    pub thread_id: String,
    pub repo_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpsertSecretCardPayload {
    pub thread_id: String,
    pub turn_item_id: String,
    pub label: String,
    #[serde(default)]
    pub placeholder: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub delivery_file: bool,
    #[serde(default)]
    pub fields: Option<Vec<SecretRequestField>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretFieldValuePayload {
    pub turn_item_id: String,
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretFieldKeyPayload {
    pub turn_item_id: String,
    pub key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnswerSecretCardPayload {
    pub turn_item_id: String,
    #[serde(default)]
    pub decline: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpsertMcpSignInPayload {
    pub thread_id: String,
    pub notice_id: String,
    pub notice: OrchestrationMcpSignInNotice,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartMcpSignInPayload {
    pub notice_id: String,
    #[serde(default)]
    pub provider_thread_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompleteMcpSignInPayload {
    pub notice_id: String,
    pub callback_url: String,
    #[serde(default)]
    pub provider_thread_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveMcpSignInPayload {
    pub notice_id: String,
    #[serde(default)]
    pub status: Option<McpSignInUiStatus>,
    #[serde(default)]
    pub contract_status: Option<McpSignInStatus>,
    #[serde(default)]
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetUpdateCapsulePayload {
    pub phase: UpdateCapsulePhase,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub progress_percent: u8,
    #[serde(default)]
    pub restart_at_ms: Option<i64>,
    #[serde(default)]
    pub now_ms: i64,
    #[serde(default)]
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncLinkedThreadPanePayload {
    #[serde(default)]
    pub working: Option<bool>,
    #[serde(default)]
    pub has_more_history: Option<bool>,
    #[serde(default)]
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeybindingListPayload {
    pub entries: Vec<KeybindingEntry>,
}
