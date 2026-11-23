//! Cursor out-of-process SDK worker framing, background shell survival/wakes,
//! Max Mode selection preservation, and one-shot runtime instruction tracking.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Cursor adapter advertises `supportsBackgroundWait: true` (`b5453de6b5`).
pub const CURSOR_SUPPORTS_BACKGROUND_WAIT: bool = true;

/// Provider-specific instruction prepended until delivered for background shell waits (`b5453de6b5`).
pub const CURSOR_BACKGROUND_WAIT_INSTRUCTIONS: &str = "To wait for a background shell you started (tests, a build), call `legion_background_wait` with its shell ID or pid before you end your turn. It returns at once, and LegionCode wakes you when the shell exits. Do not mark work you leave running, or PRs.";

/// Maximum nested cause depth serialized across the Cursor SDK worker IPC pipe (`29af131f06`).
pub const MAX_WORKER_ERROR_CAUSE_DEPTH: usize = 8;

const BACKGROUND_SHELL_DETAIL_COMMAND_MAX_CHARS: usize = 200;

/// Tagged SDK error categories preserved across worker process serialization (`29af131f06`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CursorSdkErrorKind {
    /// Missing or invalid Cursor API/account authentication.
    Auth,
    /// Quota or rate-limit rejection.
    RateLimit,
    /// Subprocess or SDK timeout.
    Timeout,
    /// Unclassified SDK failure.
    Unknown,
}

/// Depth-bounded error structure serialized over the Cursor worker NDJSON pipe (`29af131f06`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SerializedWorkerError {
    /// Error class name (`"Error"`, `"CursorAuthError"`, etc.).
    pub name: String,
    /// Human-readable error message.
    pub message: String,
    /// Optional stack trace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack: Option<String>,
    /// Optional OS/SDK error code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Optional HTTP status code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// Preserved tagged SDK error kind across IPC serialization.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sdk_error_kind: Option<CursorSdkErrorKind>,
    /// Recursive cause chain truncated at [`MAX_WORKER_ERROR_CAUSE_DEPTH`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<Box<Self>>,
}

impl SerializedWorkerError {
    /// Clamps nested `cause` chains to [`MAX_WORKER_ERROR_CAUSE_DEPTH`] before wire encoding (`29af131f06`).
    #[must_use]
    pub fn clamp_cause_depth(mut self) -> Self {
        fn trim(err: &mut SerializedWorkerError, depth: usize) {
            if depth >= MAX_WORKER_ERROR_CAUSE_DEPTH {
                err.cause = None;
            } else if let Some(inner) = err.cause.as_deref_mut() {
                trim(inner, depth + 1);
            }
        }
        trim(&mut self, 0);
        self
    }
}

/// Background shell lifecycle event emitted from the Cursor SDK worker (`29af131f06`, `b5453de6b5`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum SerializedBackgroundShellEvent {
    /// Background shell spawned and supervised for turn survival.
    #[serde(rename_all = "camelCase")]
    Started {
        id: String,
        pid: Option<u32>,
        command: String,
    },
    /// Background shell exited or was explicitly stopped.
    #[serde(rename_all = "camelCase")]
    Ended {
        id: String,
        pid: Option<u32>,
        command: String,
        stopped: bool,
        exit_code: Option<i32>,
        output_path: Option<String>,
    },
}

/// Inbound request envelope sent from host server to the Cursor SDK worker process (`29af131f06`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum CursorWorkerRequest {
    /// Opens or attaches a Cursor SDK agent session.
    #[serde(rename_all = "camelCase")]
    Open {
        id: u64,
        agent_id: Option<String>,
        thread_id: String,
        provider_session_id: String,
        options: Value,
    },
    /// Starts or resumes a Cursor turn run.
    #[serde(rename_all = "camelCase")]
    Send {
        id: u64,
        session_id: u64,
        message: Value,
        #[serde(default)]
        summarize: bool,
        #[serde(default)]
        resume: bool,
        #[serde(default)]
        listen_background_shells: bool,
    },
    /// Stops a supervised background shell by `shell_id`.
    #[serde(rename_all = "camelCase")]
    StopBackgroundShell {
        id: u64,
        session_id: u64,
        shell_id: String,
    },
    /// Delivers a mid-turn steer message.
    #[serde(rename_all = "camelCase")]
    Steer { id: u64, send_id: u64, text: String },
    /// Cancels an in-flight `send`.
    #[serde(rename_all = "camelCase")]
    Cancel { id: u64, send_id: u64 },
    /// Closes one worker session handle.
    #[serde(rename_all = "camelCase")]
    CloseSession { id: u64, session_id: u64 },
    /// Shuts down all worker sessions cleanly.
    Close { id: u64 },
}

/// Outbound frame emitted by the Cursor SDK worker process (`29af131f06`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum CursorWorkerMessage {
    /// RPC response frame (`id` matches [`CursorWorkerRequest`]).
    Response {
        id: u64,
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<SerializedWorkerError>,
    },
    /// Streaming interaction delta for an active `send_id`.
    #[serde(rename_all = "camelCase")]
    Delta { send_id: u64, update: Value },
    /// Terminal completion/failure frame for `send_id`.
    #[serde(rename_all = "camelCase")]
    RunDone {
        send_id: u64,
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<SerializedWorkerError>,
    },
    /// Background shell start/exit lifecycle notification (`b5453de6b5`).
    #[serde(rename_all = "camelCase")]
    BackgroundShell {
        session_id: u64,
        event: SerializedBackgroundShellEvent,
    },
}

/// Encodes one [`CursorWorkerMessage`] as single-line NDJSON (`29af131f06`).
pub fn encode_cursor_worker_message(
    message: &CursorWorkerMessage,
) -> Result<String, serde_json::Error> {
    let mut line = serde_json::to_string(message)?;
    line.push('\n');
    Ok(line)
}

/// Decodes one Cursor SDK worker request NDJSON line (`29af131f06`).
pub fn decode_cursor_worker_request(line: &str) -> Result<CursorWorkerRequest, serde_json::Error> {
    serde_json::from_str(line.trim())
}

/// Formats a Cursor background shell wake detail string with Unicode code-point bounding (`b5453de6b5`).
#[must_use]
pub fn cursor_background_shell_detail(
    command: &str,
    exit_code: Option<i32>,
    output_path: Option<&str>,
) -> String {
    let mut chars = command.chars();
    let prefix: String = chars
        .by_ref()
        .take(BACKGROUND_SHELL_DETAIL_COMMAND_MAX_CHARS)
        .collect();
    let bounded_cmd = if chars.next().is_some() {
        format!("{prefix}...")
    } else {
        prefix
    };
    let exit_suffix = exit_code.map_or_else(String::new, |code| format!(" (exit {code})"));
    let header = format!("Background command completed{exit_suffix}: {bounded_cmd}");
    match output_path {
        Some(path) => format!("{header}\n\nOutput: {path}"),
        None => header,
    }
}

/// Active supervised background shell metadata tracked for `legion_background_wait` (`b5453de6b5`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorTrackedBackgroundShell {
    /// Cursor shell task ID.
    pub id: String,
    /// Optional OS process identifier (also accepted by `legion_background_wait`).
    pub pid: Option<u32>,
    /// Original shell command text.
    pub command: String,
    /// Whether the turn explicitly awaited this shell via `legion_background_wait`.
    pub awaited: bool,
}

/// Matches a requested `task_ids` slice against tracked shell `id` or stringified `pid`,
/// marking matches `awaited = true` (`b5453de6b5`).
#[must_use]
pub fn await_cursor_background_shells(
    shells: &mut [CursorTrackedBackgroundShell],
    requested_targets: &[&str],
) -> (Vec<String>, Vec<String>) {
    let mut matched = Vec::new();
    let mut unmatched = Vec::new();
    for target in requested_targets {
        let trimmed = target.trim();
        let hit = shells.iter_mut().find(|shell| {
            shell.id == trimmed || shell.pid.is_some_and(|pid| pid.to_string() == trimmed)
        });
        if let Some(shell) = hit {
            shell.awaited = true;
            matched.push((*target).to_owned());
        } else {
            unmatched.push((*target).to_owned());
        }
    }
    (matched, unmatched)
}

/// Parameter entry passed to Cursor SDK model selection (`57e92b0b98`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorSdkParam {
    pub id: String,
    pub value: String,
}

/// Variant metadata exposed by `@cursor/sdk` models (`57e92b0b98`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorSdkVariant {
    pub params: Vec<CursorSdkParam>,
    pub is_max_mode: bool,
}

/// Minimal Cursor SDK catalog model metadata (`57e92b0b98`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorSdkModel {
    pub id: String,
    pub aliases: Vec<String>,
    pub variants: Vec<CursorSdkVariant>,
}

/// Resolved Cursor SDK model selection preserving Max Mode across model switches (`57e92b0b98`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorSdkModelSelection {
    pub id: String,
    pub params: Vec<CursorSdkParam>,
    pub max_mode: bool,
}

/// Builds a [`CursorSdkModelSelection`] from explicit/default options while deriving
/// `max_mode` from SDK variant metadata so switching models preserves Max Mode (`57e92b0b98`).
#[must_use]
pub fn cursor_sdk_model_selection(
    model: &str,
    params: Vec<CursorSdkParam>,
    sdk_models: &[CursorSdkModel],
) -> CursorSdkModelSelection {
    let sdk_model = sdk_models
        .iter()
        .find(|m| m.id == model || m.aliases.iter().any(|alias| alias == model));
    let max_mode = !params.is_empty()
        && sdk_model.is_some_and(|m| {
            let matching: Vec<&CursorSdkVariant> = m
                .variants
                .iter()
                .filter(|variant| {
                    params.iter().all(|p| {
                        variant.params.iter().any(|cand| {
                            cand.id.trim() == p.id.trim() && cand.value.trim() == p.value.trim()
                        })
                    })
                })
                .collect();
            !matching.is_empty() && matching.iter().all(|v| v.is_max_mode)
        });
    CursorSdkModelSelection {
        id: if model == "auto" {
            "default".to_owned()
        } else {
            model.to_owned()
        },
        params,
        max_mode,
    }
}

/// Tracks per-agent delivery of Cursor runtime instructions so instructions are emitted
/// only once per conversation and resent after compaction, rewind, or model change (`ed203c46cb`).
#[derive(Debug, Clone, Default)]
pub struct CursorRuntimeInstructionTracker {
    delivered_by_agent: BTreeMap<String, String>,
}

impl CursorRuntimeInstructionTracker {
    /// Creates an empty tracker.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `true` when `instructions` differs from what was last completed on `agent_id`.
    #[must_use]
    pub fn should_send(&self, agent_id: &str, instructions: &str) -> bool {
        self.delivered_by_agent.get(agent_id).map(String::as_str) != Some(instructions)
    }

    /// Records successful delivery ONLY when the run finished `completed` without compacting.
    pub fn record_run_outcome(
        &mut self,
        agent_id: &str,
        sent_instructions: Option<&str>,
        completed: bool,
        compacted: bool,
    ) {
        if compacted {
            self.delivered_by_agent.remove(agent_id);
        } else if completed && let Some(text) = sent_instructions {
            self.delivered_by_agent
                .insert(agent_id.to_owned(), text.to_owned());
        }
    }

    /// Clears recorded delivery on compaction or thread rewind (`ed203c46cb`).
    pub fn invalidate(&mut self, agent_id: &str) {
        self.delivered_by_agent.remove(agent_id);
    }
}

/// Returns `true` when a Cursor turn failed or was cancelled before saving any checkpoint
/// (meaning Cursor dropped the prompt from native conversation history and inline handoff should
/// preserve it as undelivered) (`#40670`).
#[must_use]
pub fn cursor_turn_prompt_dropped(
    status: &str,
    compacted: bool,
    has_prompt: bool,
    run_checkpointed: bool,
) -> bool {
    (status == "failed" || status == "cancelled") && !compacted && has_prompt && !run_checkpointed
}

/// Returns `true` when a recorded provider turn actually delivered its prompt to native history
/// (`#40670`).
#[must_use]
pub const fn is_delivered_provider_turn(prompt_dropped: bool) -> bool {
    !prompt_dropped
}
