//! Request and response DTOs for [`crate::bridge_client::ClientCoreBridge`].

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use t3_client_core::probe::{ConnectionRouteKind, ProbeAction, ProbeSignal};
use t3_client_core::steady_shells::{EnvironmentShellStatus, SteadyShell};
use t3_client_core::thread_families::{ThreadFamilyRead, ThreadFamilyRow};
use t3_contracts::ids::{EnvironmentId, ThreadId};

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct SteadyShellInputDto {
    #[serde(alias = "environment_id")]
    pub environment_id: String,
    pub snapshot: Option<serde_json::Value>,
    #[serde(alias = "server_sequence", alias = "snapshotSequence")]
    pub server_sequence: Option<u64>,
    pub status: Option<EnvironmentShellStatus>,
    pub generation: u64,
    pub connecting: Option<bool>,
    #[serde(alias = "row_actions")]
    pub row_actions: u64,
    pub remove: bool,
    #[serde(alias = "now_ms")]
    pub now_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum SteadyShellEnqueuePayload {
    BatchList(Vec<SteadyShellInputDto>),
    BatchObject {
        inputs: Vec<SteadyShellInputDto>,
        #[serde(default, rename = "nowMs", alias = "now_ms")]
        now_ms: Option<u64>,
    },
    Single(Box<SteadyShellInputDto>),
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SteadyShellStepResponse<'a> {
    pub now_ms: u64,
    pub ready: bool,
    pub changed: bool,
    pub holding: bool,
    pub release_at_ms: Option<u64>,
    pub timer_token: Option<u64>,
    pub shells: &'a BTreeMap<EnvironmentId, SteadyShell<serde_json::Value>>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum FamilyRowInput {
    IdOnly(String),
    WithPayload {
        id: String,
        #[serde(default)]
        payload: serde_json::Value,
    },
}

impl FamilyRowInput {
    pub fn into_row(self) -> Result<ThreadFamilyRow<serde_json::Value>, String> {
        match self {
            Self::IdOnly(raw_id) => {
                let id = ThreadId::new(raw_id).map_err(|err| err.to_string())?;
                Ok(ThreadFamilyRow::new(id, serde_json::Value::Null))
            }
            Self::WithPayload { id, payload } => {
                let tid = ThreadId::new(id).map_err(|err| err.to_string())?;
                Ok(ThreadFamilyRow::new(tid, payload))
            }
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct TouchFamilyRequest {
    pub action: Option<String>,
    #[serde(alias = "environment_id")]
    pub environment_id: Option<String>,
    #[serde(alias = "thread_id")]
    pub thread_id: Option<String>,
    pub generation: Option<u64>,
    #[serde(alias = "now_ms")]
    pub now_ms: Option<u64>,
    #[serde(alias = "snapshot_sequence")]
    pub snapshot_sequence: Option<u64>,
    pub threads: Option<Vec<FamilyRowInput>>,
    pub error: Option<String>,
    #[serde(alias = "shell_thread_families_enabled")]
    pub enabled: Option<bool>,
    #[serde(alias = "shell_threads")]
    pub shell_threads: Option<Vec<FamilyRowInput>>,
    #[serde(alias = "older_settled_as_of_sequence")]
    pub older_settled_as_of_sequence: Option<u64>,
    #[serde(alias = "removed_at")]
    pub removed_at: BTreeMap<String, u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TouchFamilyResponse {
    pub action: String,
    pub environment_id: Option<EnvironmentId>,
    pub thread_id: Option<ThreadId>,
    pub shell_generation: Option<u64>,
    pub read_state: Option<ThreadFamilyRead>,
    pub mount_ref_count: Option<usize>,
    pub cached_entry_count: usize,
    pub fetch_generation: Option<u64>,
    pub released: Option<bool>,
    pub evicted_count: Option<usize>,
    pub merged_threads: Option<Vec<ThreadFamilyRow<serde_json::Value>>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DisconnectDecisionResponse {
    pub now_ms: u64,
    pub attempt: u32,
    pub delay_ms: u64,
    pub next_retry_at_ms: u64,
    pub awaiting_approval: bool,
    pub first_loss: bool,
    pub failure_count: u32,
    pub backoff_failures: u32,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(crate) enum ProbeStepRequest {
    Reset {
        #[serde(default, rename = "routeKind", alias = "route_kind")]
        route_kind: Option<ConnectionRouteKind>,
    },
    Signal {
        signal: ProbeSignal,
        #[serde(default, rename = "nowMs", alias = "now_ms")]
        now_ms: Option<u64>,
    },
    Tick {
        #[serde(default, rename = "nowMs", alias = "now_ms")]
        now_ms: Option<u64>,
        #[serde(default, rename = "currentFrames", alias = "current_frames")]
        current_frames: u64,
    },
    #[serde(alias = "config_loaded")]
    ConfigLoaded {
        loaded: bool,
        #[serde(default, rename = "currentFrames", alias = "current_frames")]
        current_frames: u64,
        #[serde(default, rename = "nowMs", alias = "now_ms")]
        now_ms: Option<u64>,
    },
    #[serde(alias = "frames_observed")]
    FramesObserved {
        #[serde(rename = "currentFrames", alias = "current_frames")]
        current_frames: u64,
        #[serde(default, rename = "nowMs", alias = "now_ms")]
        now_ms: Option<u64>,
    },
    #[serde(alias = "host_check_result")]
    HostCheckResult {
        answered: bool,
        #[serde(default, rename = "currentFrames", alias = "current_frames")]
        current_frames: u64,
        #[serde(default, rename = "nowMs", alias = "now_ms")]
        now_ms: Option<u64>,
    },
    #[serde(alias = "probe_completed")]
    ProbeCompleted {
        succeeded: bool,
    },
    Status,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProbeStepResponse {
    pub actions: Vec<ProbeAction>,
    pub probing: bool,
    pub next_deadline_ms: Option<u64>,
}
