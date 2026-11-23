//! Antigravity turn lifecycle, sleep-safe timeout, child-work, failure, and text-bounding helpers.

use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use legion_contracts::brand::ACTIVE_BRAND;
use serde::Deserialize;
use serde_json::Value;

use crate::acp::ToolCallKind;

pub use super::failure_bounds::{
    AntigravityTurnFailure, bound_antigravity_command_text, classify_antigravity_turn_failure,
    sanitize_antigravity_command_fields,
};

/// Inactivity timeout sleep slice duration (`990f31cac2`).
pub const ACP_PROMPT_INACTIVITY_SLICE_MILLIS: i64 = 60_000;

/// Host suspend/wake overrun tolerance beyond a sleep slice (`990f31cac2`).
pub const ACP_HOST_SLEEP_TOLERANCE_MILLIS: i64 = 10_000;

/// Native trajectory step type indicating the model produced a response in this turn (`f68d36d8aa`).
pub const PLANNER_RESPONSE_STEP_TYPE: u32 = 15;

const HIGH_PRIORITY_MESSAGE: &str = "MESSAGE_PRIORITY_HIGH";
const SUBAGENT_STATE_KILLED: &str = "SUBAGENT_STATE_KILLED";

const SCHEDULE_TITLES: &[&str] = &["Run schedule", "Running schedule"];

/// Outcome of one sleep-safe inactivity timeout check (`990f31cac2`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InactivitySliceAction {
    /// Sleep the returned bounded duration (capped at [`ACP_PROMPT_INACTIVITY_SLICE_MILLIS`]).
    SleepSlice { slice_millis: i64 },
    /// Host sleep jumped past `slice + tolerance`; reset `last_progress_at = now_millis`.
    ResetProgressAfterHostSleep { new_last_progress_at: i64 },
    /// Real inactivity timeout elapsed without host suspend distortion.
    TimedOut,
}

/// Evaluates whether the ACP turn inactivity timer should sleep another slice, reset
/// after host suspend/wake, or fire timeout (`990f31cac2`).
#[must_use]
pub const fn evaluate_turn_inactivity_slice(
    last_progress_at_ms: i64,
    timeout_ms: i64,
    slice_started_at_ms: Option<i64>,
    previous_slice_ms: i64,
    now_ms: i64,
) -> InactivitySliceAction {
    if let Some(started_ms) = slice_started_at_ms
        && now_ms - started_ms > previous_slice_ms + ACP_HOST_SLEEP_TOLERANCE_MILLIS
    {
        return InactivitySliceAction::ResetProgressAfterHostSleep {
            new_last_progress_at: now_ms,
        };
    }
    let remaining = last_progress_at_ms + timeout_ms - now_ms;
    if remaining <= 0 {
        return InactivitySliceAction::TimedOut;
    }
    let slice = if remaining < ACP_PROMPT_INACTIVITY_SLICE_MILLIS {
        remaining
    } else {
        ACP_PROMPT_INACTIVITY_SLICE_MILLIS
    };
    InactivitySliceAction::SleepSlice {
        slice_millis: slice,
    }
}

/// Returns `true` when `tool_name` identifies `LegionCode` background-wait MCP tooling (`505f6d6cf4`).
///
/// Accepts `legion_background_wait`, or MCP-prefixed forms resolved through [`ACTIVE_BRAND`].
#[must_use]
pub fn is_antigravity_background_wait_tool(tool_name: &str) -> bool {
    let trimmed = tool_name.trim();
    if trimmed == "legion_background_wait" {
        return true;
    }
    trimmed != "background_wait"
        && ACTIVE_BRAND.canonical_mcp_tool_suffix(trimmed) == "background_wait"
}

/// Bounds how long a settled Antigravity turn can stay held by silent background commands (`30 minutes`, `#40858`).
pub const ANTIGRAVITY_POST_SETTLE_COMMAND_HOLD_MILLIS: i64 = 30 * 60 * 1_000;

/// Returns `true` when an Antigravity tool call continues running after its prompt returns
/// (scheduled wakes or non-daemon shell commands) (`505f6d6cf4`, `6edb53026b`).
#[must_use]
pub fn antigravity_tool_outlives_prompt(
    kind: &ToolCallKind,
    title: Option<&str>,
    raw_input: Option<&Value>,
) -> bool {
    if let Some(t) = title
        && SCHEDULE_TITLES.contains(&t)
    {
        return true;
    }
    antigravity_command_outlives_prompt(kind, title, raw_input)
}

/// Returns `true` for non-daemon shell commands awaited across prompt settle (`#40858`).
///
/// Unlike scheduled wakes, commands report progress while running and are subject to the
/// 30-minute silent post-settle hold expiry.
#[must_use]
pub fn antigravity_command_outlives_prompt(
    kind: &ToolCallKind,
    title: Option<&str>,
    raw_input: Option<&Value>,
) -> bool {
    if let Some(t) = title
        && SCHEDULE_TITLES.contains(&t)
    {
        return false;
    }
    if *kind != ToolCallKind::Execute {
        return false;
    }
    let is_daemon = raw_input
        .and_then(Value::as_object)
        .and_then(|obj| obj.get("IsDaemon"))
        .and_then(Value::as_bool)
        == Some(true);
    !is_daemon
}

/// Prompt lifecycle phase evaluated when checking background command holds (`#40858`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AntigravityTurnPhase {
    /// Model prompt has not settled yet.
    ActivePrompt,
    /// Prompt settled while non-daemon background commands remain alive.
    SettledPrompt,
    /// Turn has already finalized.
    Finalized,
}

/// Inputs evaluated for the silent post-settle background command hold timer (`#40858`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PostSettleCommandHoldState {
    /// Current lifecycle phase of the turn.
    pub phase: AntigravityTurnPhase,
    /// Start of the current quiet window (epoch ms).
    pub quiet_since_ms: i64,
    /// Timestamp of the last observed tool or model activity (epoch ms).
    pub last_progress_at_ms: i64,
    /// Current wall-clock timestamp (epoch ms).
    pub now_ms: i64,
    /// Maximum silent hold duration before finalizing (ms).
    pub hold_timeout_ms: i64,
    /// Whether any non-daemon background command is still holding the turn.
    pub has_running_command: bool,
    /// Whether non-command holds (questions, schedules, subagents) are active.
    pub has_other_deferred_work: bool,
}

/// Decision for the silent post-settle background command hold timer (`#40858`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostSettleCommandHoldAction {
    /// Prompt is not settled or turn is already finalized; stop the expiry timer.
    StopTimer,
    /// Remaining milliseconds to sleep before re-checking silence.
    SleepRemaining { remaining_ms: i64 },
    /// Other background holds (questions, schedules, subagents) are active; reset `quiet_since_ms = now_ms`.
    ResetSilence { new_quiet_since_ms: i64 },
    /// Silence threshold exceeded with only running commands holding the turn; finalize now.
    FinalizeTurn,
}

/// Evaluates whether a settled turn held only by background commands has gone silent past
/// `hold_timeout_ms` (`#40858`).
#[must_use]
pub fn evaluate_post_settle_command_hold(
    state: &PostSettleCommandHoldState,
) -> PostSettleCommandHoldAction {
    if state.phase != AntigravityTurnPhase::SettledPrompt {
        return PostSettleCommandHoldAction::StopTimer;
    }
    let effective_quiet = state.quiet_since_ms.max(state.last_progress_at_ms);
    let remaining_ms = effective_quiet
        .saturating_add(state.hold_timeout_ms)
        .saturating_sub(state.now_ms);
    if remaining_ms > 0 {
        return PostSettleCommandHoldAction::SleepRemaining { remaining_ms };
    }
    if state.has_running_command && !state.has_other_deferred_work {
        PostSettleCommandHoldAction::FinalizeTurn
    } else {
        PostSettleCommandHoldAction::ResetSilence {
            new_quiet_since_ms: state.now_ms,
        }
    }
}

/// Returns `true` when a worktree-handoff thread settling to idle must finalize its
/// open turn instead of staying pinned in `Waiting` (`9f6f5540cf`).
#[must_use]
pub const fn should_finalize_worktree_handoff_turn(
    has_open_turn: bool,
    settled_idle: bool,
    has_pending_child_work: bool,
) -> bool {
    has_open_turn && settled_idle && !has_pending_child_work
}

/// Returns `true` when an active Antigravity run can accept a mid-turn steer,
/// including schedule/background wake runs (`prompt_sequence == 0`) (`e24f0c7154`).
#[must_use]
pub const fn can_steer_antigravity_turn(
    has_active_session: bool,
    prompt_in_flight: bool,
    prompt_sequence: u64,
) -> bool {
    let _ = prompt_sequence;
    has_active_session && prompt_in_flight
}

/// Returns `true` when a subagent record state represents a permanently killed helper (`194726492b`).
#[must_use]
pub fn is_antigravity_subagent_killed(state: Option<&str>) -> bool {
    state == Some(SUBAGENT_STATE_KILLED)
}

#[derive(Debug, Deserialize)]
struct InboxMessageMeta {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    priority: Option<String>,
}

/// Inspects an Antigravity agent `messages/` directory for any unread high-priority
/// inbox message that will wake the agent (`3a91757430`).
#[must_use]
pub fn has_unread_waking_inbox_message(messages_dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(messages_dir) else {
        return false;
    };
    let read_receipts: BTreeMap<String, bool> =
        std::fs::read_to_string(messages_dir.join("read.json"))
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();

    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else {
            continue;
        };
        if !Path::new(name_str)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
            || name_str == "read.json"
        {
            continue;
        }
        let file_id = name_str.strip_suffix(".json").unwrap_or(name_str);
        if read_receipts.get(file_id).copied() == Some(true) {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        let Ok(msg) = serde_json::from_str::<InboxMessageMeta>(&raw) else {
            continue;
        };
        if let Some(id) = msg.id.as_deref()
            && read_receipts.get(id).copied() == Some(true)
        {
            continue;
        }
        if msg.priority.as_deref() == Some(HIGH_PRIORITY_MESSAGE) {
            return true;
        }
    }
    false
}

/// Tail read result from `transcript.jsonl` preserving exact step line boundaries (`ad4d0f0659`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompleteJsonlTail {
    /// Complete UTF-8 JSONL lines inside the tail window.
    pub lines: Vec<String>,
    /// `true` when the byte cap clipped earlier file bytes.
    pub truncated: bool,
}

/// Reads the trailing `byte_cap` slice of a JSONL transcript while peeking 1 extra leading
/// byte when truncated so a mid-line boundary is never parsed as a partial record (`ad4d0f0659`).
pub fn read_complete_jsonl_tail(
    path: &Path,
    byte_cap: usize,
) -> std::io::Result<CompleteJsonlTail> {
    let mut file = std::fs::File::open(path)?;
    let size = usize::try_from(file.metadata()?.len()).unwrap_or(usize::MAX);
    if size == 0 || byte_cap == 0 {
        return Ok(CompleteJsonlTail {
            lines: Vec::new(),
            truncated: size > 0,
        });
    }
    let tail_start = size.saturating_sub(byte_cap);
    let read_start = tail_start.saturating_sub(1);
    file.seek(SeekFrom::Start(
        u64::try_from(read_start).unwrap_or(u64::MAX),
    ))?;
    let mut bytes = Vec::with_capacity(size.saturating_sub(read_start));
    file.read_to_end(&mut bytes)?;
    let start_offset = if tail_start > 0 {
        let Some(nl) = bytes.iter().position(|&b| b == b'\n') else {
            return Ok(CompleteJsonlTail {
                lines: Vec::new(),
                truncated: true,
            });
        };
        nl + 1
    } else {
        0
    };
    let text = String::from_utf8_lossy(&bytes[start_offset..]);
    let lines = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    Ok(CompleteJsonlTail {
        lines,
        truncated: tail_start > 0,
    })
}
