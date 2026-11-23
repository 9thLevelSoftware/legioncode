//! ACP session-load command/replay synchronization, post-turn usage persistence,
//! and PowerShell-safe terminal MCP fallback launcher/argument parsing.

use std::collections::BTreeMap;

use serde_json::Value;

use super::updates::AvailableCommand;

/// Startup JSON-RPC handshake timeout (`90 seconds`) covering `initialize`, `session/new`,
/// and `session/resume` so a wedged child process does not hang startup forever (`#40864`).
pub const ACP_STARTUP_HANDSHAKE_TIMEOUT_MS: u64 = 90_000;

/// Bounded wait (`2 seconds`) for `available_commands_update` immediately after `session/load` (`6d6ca60907`).
pub const ACP_LOADED_COMMANDS_WAIT_MS: u64 = 2_000;

/// Windows cmd wrapper letting PowerShell callers await and read `acp-mcp-call` exit codes (`4b0dd9eb1c`).
pub const ACP_MCP_CALL_LAUNCHER_CMD: &str = concat!(
    "@echo off\r\n",
    "setlocal EnableExtensions DisableDelayedExpansion\r\n",
    "set \"ELECTRON_RUN_AS_NODE=1\"\r\n",
    "if not defined LEGIONCODE_ACP_MCP_ENTRYPOINT goto packaged\r\n",
    "\"%LEGIONCODE_ACP_MCP_NODE%\" \"%LEGIONCODE_ACP_MCP_ENTRYPOINT%\" acp-mcp-call %*\r\n",
    "exit /b %errorlevel%\r\n",
    ":packaged\r\n",
    "\"%LEGIONCODE_ACP_MCP_NODE%\" acp-mcp-call %*\r\n",
);

/// Result of checking whether `/compact` should run, wait for loaded commands, or fail (`6d6ca60907`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpCompactionGate {
    /// Session advertised `"compact"`, so proceed immediately.
    Allowed,
    /// `session/load` has not yet emitted `available_commands_update`; wait up to [`ACP_LOADED_COMMANDS_WAIT_MS`].
    AwaitCommandsUpdate,
    /// Advertised command list does not contain `"compact"`.
    Unsupported,
}

/// Checks whether an ACP `/compact` message can execute or should await `available_commands_update`
/// after `session/load` (`6d6ca60907`).
#[must_use]
pub fn evaluate_acp_compaction_gate(
    compact_via_advertised_command: bool,
    is_compaction_message: bool,
    advertised_commands: Option<&[AvailableCommand]>,
) -> AcpCompactionGate {
    if !is_compaction_message || !compact_via_advertised_command {
        return AcpCompactionGate::Allowed;
    }
    let Some(commands) = advertised_commands else {
        return AcpCompactionGate::AwaitCommandsUpdate;
    };
    if commands.iter().any(|cmd| cmd.name == "compact") {
        AcpCompactionGate::Allowed
    } else {
        AcpCompactionGate::Unsupported
    }
}

/// Returns `true` only for live session updates (never replayed history during `session/load`)
/// so replayed background tool text never pins background work (`0c921e1fdf`).
#[must_use]
pub const fn acp_session_update_applies_live_background_mutations(
    is_replay: bool,
    live: bool,
) -> bool {
    !is_replay && live
}

/// Normalized ACP `UsageUpdate` emission carrying `session_scoped` persistence flag (`baf7e5b15a`).
#[derive(Debug, Clone, PartialEq)]
pub struct AcpUsageUpdateEmission {
    pub used: Option<u64>,
    pub size: Option<u64>,
    pub cost: Option<Value>,
    /// Set `true` when emitted outside an active turn so session store persists post-turn context usage.
    pub session_scoped: bool,
}

/// Stamps `session_scoped = !turn_active` on `UsageUpdate` notifications (`baf7e5b15a`).
#[must_use]
pub fn format_acp_usage_update_emission(
    used: Option<u64>,
    size: Option<u64>,
    cost: Option<Value>,
    turn_active: bool,
) -> AcpUsageUpdateEmission {
    AcpUsageUpdateEmission {
        used,
        size,
        cost,
        session_scoped: !turn_active,
    }
}

/// Parses `acp-mcp-call` JSON arguments, reading from `stdin` when `arguments_arg == "-"` so
/// Windows PowerShell 5.1 callers avoid CLI quote stripping (`4b0dd9eb1c`).
pub fn resolve_acp_mcp_call_arguments(
    arguments_arg: &str,
    stdin_payload: &str,
) -> Result<Value, serde_json::Error> {
    let raw = if arguments_arg == "-" {
        stdin_payload
    } else {
        arguments_arg
    };
    serde_json::from_str(raw)
}

/// Formats the timeout error detail when an ACP handshake RPC (`initialize`, `session/new`,
/// `session/resume`) exceeds [`ACP_STARTUP_HANDSHAKE_TIMEOUT_MS`] (`#40864`).
#[must_use]
pub fn acp_startup_timeout_error_detail(method: &str) -> String {
    format!("{method} timed out waiting for RPC response")
}

/// Preserves `"idle"` thread status when a late post-stop `session/update` (such as usage telemetry)
/// arrives while no turn is active (`context == null`) and the cached snapshot still says `"active"` (`#40629`).
#[must_use]
pub fn acp_late_session_update_thread_status(has_active_turn: bool, cached_status: &str) -> &str {
    if !has_active_turn && cached_status == "active" {
        "idle"
    } else {
        cached_status
    }
}

/// Confirms an adapter interrupt exit during turn ingestion should be caught unconditionally
/// (even interrupt-only causes caused by closing an ACP session scope) so the run settles
/// instead of staying `running` (`#40734`).
#[must_use]
pub const fn acp_interrupt_cause_should_settle_turn(session_scope_closing: bool) -> bool {
    let _ = session_scope_closing;
    true
}

/// Mode delta emitted when an ACP agent exits a mode (such as leaving Plan Mode after
/// approval) on its own (`#40818`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpAgentModeChange {
    /// Option identifier (`"mode"`, `"collaboration_mode"`, or `configOption.id`).
    pub id: String,
    /// Previously selected option value on the thread.
    pub from: String,
    /// Live mode value reported by the agent.
    pub to: String,
}

/// Detects mode changes initiated by the agent since `configureSession` set `configured_modes` (`#40818`).
///
/// Only values that changed relative to BOTH `configured_modes` and the thread's current
/// `selected_options` are reported so picking an exited mode again registers as a fresh change.
#[must_use]
pub fn detect_acp_agent_mode_changes(
    configured_modes: &BTreeMap<String, String>,
    live_modes: &BTreeMap<String, String>,
    selected_options: &[(&str, &str)],
) -> Vec<AcpAgentModeChange> {
    let mut changes = Vec::new();
    for &(option_id, selected_value) in selected_options {
        let Some(live_value) = live_modes.get(option_id) else {
            continue;
        };
        if configured_modes.get(option_id).map(String::as_str) == Some(live_value.as_str())
            || live_value == selected_value
        {
            continue;
        }
        changes.push(AcpAgentModeChange {
            id: option_id.to_owned(),
            from: selected_value.to_owned(),
            to: live_value.clone(),
        });
    }
    changes
}
