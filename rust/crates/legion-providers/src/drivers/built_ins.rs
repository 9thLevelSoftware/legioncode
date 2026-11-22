//! Built-in specifications and command/probe/env builders for every `LegionCode` driver.
//!
//! Exports `codex`, `claude`, `cursor`, `kiro`, `fx`, and re-exports `opencode`,
//! `pi`, `muse`, and `acp_registry` helpers from [`super::built_ins_extra`].
//! All environment builders take `&SanitizedEnv` (`BUG-12`).

use std::collections::BTreeMap;

use legion_protocol::provider_env::SanitizedEnv;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::acp::{AcpCodecError, AcpStdioCodec, JsonRpcId};

pub use super::built_ins_acp::{
    FX_ACP_CANCEL_BEHAVIOR, FxLaunchSpec, KIRO_AUTOPILOT_CONFIG_ID, KIRO_EFFORT_CONFIG_ID,
    KiroLaunchSpec, fx_launch_spec, fx_session_mode_for_policy, fx_tool_call_was_cancelled,
    kiro_autopilot_value, kiro_launch_spec,
};
pub use super::built_ins_extra::{
    MUSE_PROTOCOL_VERSION, PiLaunchError, RegistryExecError, fill_compaction_recents,
    muse_launch_spec, muse_verbatim_path, normalize_registry_command_path,
    opencode_basic_auth_header, opencode_server_env, pi_launch_args, pi_probe_spec,
    validate_registry_executable,
};
use super::{ProviderDriverSpec, build_child_env};

/// Host session variables stripped when spawning isolated Claude Code subprocesses.
pub const CLAUDE_HOST_SESSION_VARIABLES: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_SSE_PORT",
    "CLAUDE_CODE_ENTRYPOINT",
    "ELECTRON_RUN_AS_NODE",
];

/// Registry of all built-in driver specifications supported by `LegionCode`.
pub const BUILTIN_DRIVER_SPECS: &[ProviderDriverSpec] = &[
    ProviderDriverSpec {
        slug: "codex",
        display_name: "Codex",
        default_binary: "codex",
        supports_acp: false,
        supports_text_generation: true,
        cancel_behavior: "interrupt",
    },
    ProviderDriverSpec {
        slug: "claudeAgent",
        display_name: "Claude Code",
        default_binary: "claude",
        supports_acp: false,
        supports_text_generation: true,
        cancel_behavior: "interrupt",
    },
    ProviderDriverSpec {
        slug: "cursor",
        display_name: "Cursor",
        default_binary: "cursor-agent",
        supports_acp: false,
        supports_text_generation: true,
        cancel_behavior: "interrupt",
    },
    ProviderDriverSpec {
        slug: "grok",
        display_name: "Grok",
        default_binary: "grok",
        supports_acp: true,
        supports_text_generation: true,
        cancel_behavior: "ctrl_c",
    },
    ProviderDriverSpec {
        slug: "opencode",
        display_name: "OpenCode",
        default_binary: "opencode",
        supports_acp: false,
        supports_text_generation: true,
        cancel_behavior: "interrupt",
    },
    ProviderDriverSpec {
        slug: "antigravity",
        display_name: "Antigravity",
        default_binary: "agy",
        supports_acp: true,
        supports_text_generation: true,
        cancel_behavior: "immediate",
    },
    ProviderDriverSpec {
        slug: "kiro",
        display_name: "Kiro",
        default_binary: "kiro-cli",
        supports_acp: true,
        supports_text_generation: true,
        cancel_behavior: "immediate",
    },
    ProviderDriverSpec {
        slug: "pi",
        display_name: "Pi",
        default_binary: "pi",
        supports_acp: false,
        supports_text_generation: true,
        cancel_behavior: "interrupt",
    },
    ProviderDriverSpec {
        slug: "muse",
        display_name: "Muse Code",
        default_binary: "muse",
        supports_acp: false,
        supports_text_generation: false,
        cancel_behavior: "immediate",
    },
    ProviderDriverSpec {
        slug: "fx",
        display_name: "fx",
        default_binary: "fx",
        supports_acp: true,
        supports_text_generation: false,
        cancel_behavior: FX_ACP_CANCEL_BEHAVIOR,
    },
    ProviderDriverSpec {
        slug: "acpRegistry",
        display_name: "ACP Registry",
        default_binary: "acp",
        supports_acp: true,
        supports_text_generation: false,
        cancel_behavior: "immediate",
    },
    ProviderDriverSpec {
        slug: "devReplay",
        display_name: "Replay (dev)",
        default_binary: "dev-replay",
        supports_acp: true,
        supports_text_generation: false,
        cancel_behavior: "immediate",
    },
];

/// Builds Codex app-server CLI launch arguments (`app-server -c thread_unload_delay_secs=0 ...`).
#[must_use]
pub fn codex_app_server_args(config_overrides: &[String]) -> Vec<String> {
    let mut args = vec![
        "app-server".to_owned(),
        "-c".to_owned(),
        "thread_unload_delay_secs=0".to_owned(),
    ];
    for item in config_overrides {
        let trimmed = item.trim();
        if !trimmed.is_empty() {
            args.push("-c".to_owned());
            args.push(trimmed.to_owned());
        }
    }
    args
}

/// Builds isolated ephemeral read-only arguments for Codex text generation (`title`/`commit`/`PR`).
#[must_use]
pub fn codex_text_gen_args(
    schema_path: &str,
    output_last_message_path: &str,
    model: Option<&str>,
    profile: Option<&str>,
) -> Vec<String> {
    let mut args = vec![
        "exec".to_owned(),
        "--ephemeral".to_owned(),
        "--skip-git-repo-check".to_owned(),
        "-s".to_owned(),
        "read-only".to_owned(),
    ];
    if let Some(m) = model.map(str::trim).filter(|s| !s.is_empty()) {
        args.push("--model".to_owned());
        args.push(m.to_owned());
    }
    if let Some(p) = profile.map(str::trim).filter(|s| !s.is_empty()) {
        args.push("--profile".to_owned());
        args.push(p.to_owned());
    }
    args.push("--output-schema".to_owned());
    args.push(schema_path.to_owned());
    args.push("--output-last-message".to_owned());
    args.push(output_last_message_path.to_owned());
    args.push("-".to_owned());
    args
}

/// Encodes a Codex app-server `thread/start` NDJSON line via [`AcpStdioCodec`].
pub fn codex_thread_start_envelope(
    codec: &mut AcpStdioCodec,
    cwd: &str,
    model: Option<&str>,
    approval_policy: &str,
    sandbox_mode: &str,
) -> Result<(JsonRpcId, String), AcpCodecError> {
    let params = json!({
        "cwd": cwd,
        "model": model,
        "approvalPolicy": approval_policy,
        "sandbox": sandbox_mode,
        "experimentalRawEvents": false,
    });
    codec.encode_request("thread/start", Some(&params))
}

/// Encodes a Codex app-server `turn/start` NDJSON line via [`AcpStdioCodec`].
pub fn codex_turn_start_envelope(
    codec: &mut AcpStdioCodec,
    thread_id: &str,
    prompt_text: &str,
) -> Result<(JsonRpcId, String), AcpCodecError> {
    let params = json!({
        "threadId": thread_id,
        "input": [{ "type": "text", "text": prompt_text }],
    });
    codec.encode_request("turn/start", Some(&params))
}

/// Errors raised while resolving a direct Claude Code binary from Windows shims.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ClaudeExecutableError {
    /// Windows `.cmd`/`.bat`/`.ps1` shim lacked a resolvable sibling native `claude.exe` or `cli.js`.
    #[error("Claude Windows shim `{shim}` could not be resolved to an underlying executable")]
    UnresolvableWindowsShim {
        /// Original shim path.
        shim: String,
    },
}

/// Resolves Win32 npm/WinGet script shims (`.cmd`, `.bat`, `.ps1`) to a direct
/// underlying binary (`claude.exe` or `cli.js`) so `cmd.exe` never intercepts arguments.
pub fn resolve_claude_executable<F>(
    binary_path: &str,
    is_windows: bool,
    path_exists: F,
) -> Result<String, ClaudeExecutableError>
where
    F: Fn(&str) -> bool,
{
    let trimmed = binary_path.trim();
    if !is_windows {
        return Ok(trimmed.to_owned());
    }
    let is_shim = std::path::Path::new(trimmed)
        .extension()
        .is_some_and(|ext| {
            ext.eq_ignore_ascii_case("cmd")
                || ext.eq_ignore_ascii_case("bat")
                || ext.eq_ignore_ascii_case("ps1")
        });
    if !is_shim {
        return Ok(trimmed.to_owned());
    }

    let dir = trimmed
        .rsplit_once(['\\', '/'])
        .map_or(".", |(parent, _)| parent);
    let candidates = [
        format!(r"{dir}\claude.exe"),
        format!(r"{dir}\node_modules\@anthropic-ai\claude-code\bin\claude.exe"),
        format!(r"{dir}\node_modules\@anthropic-ai\claude-code\cli.js"),
    ];
    for candidate in candidates {
        if path_exists(&candidate) {
            return Ok(candidate);
        }
    }
    Err(ClaudeExecutableError::UnresolvableWindowsShim {
        shim: trimmed.to_owned(),
    })
}

/// Builds sanitized child env for Claude Code (`BUG-12`), stripping host session vars
/// and setting `CLAUDE_CODE_ENTRYPOINT=legioncode`.
#[must_use]
pub fn build_claude_env(env: &SanitizedEnv, config_dir: Option<&str>) -> BTreeMap<String, String> {
    let mut overrides = BTreeMap::new();
    overrides.insert("CLAUDE_CODE_ENTRYPOINT".to_owned(), "legioncode".to_owned());
    if let Some(dir) = config_dir.map(str::trim).filter(|s| !s.is_empty()) {
        overrides.insert("CLAUDE_CONFIG_DIR".to_owned(), dir.to_owned());
    }
    build_child_env(env, CLAUDE_HOST_SESSION_VARIABLES, &overrides)
}

/// Isolated Cursor sidecar launch descriptor (`SqliteLocalAgentStore` + `settingSources`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CursorSidecarSpec {
    /// Executable path.
    pub executable: String,
    /// Isolated local store directory (`SqliteLocalAgentStore`).
    pub state_root: String,
    /// Setting sources passed to Cursor Agent SDK (`[]` when isolated for text generation).
    pub setting_sources: Vec<String>,
    /// Forces HTTP/1.1 transport when proxy compatibility requires it.
    pub use_http1_for_agent: bool,
    /// Final sanitized environment map (`BUG-12`).
    pub env: BTreeMap<String, String>,
}

/// Builds an isolated Cursor sidecar specification from `&SanitizedEnv` (`BUG-12`).
#[must_use]
pub fn cursor_isolated_sidecar_spec(
    env: &SanitizedEnv,
    executable: &str,
    state_root: &str,
    is_text_generation: bool,
    use_http1_for_agent: bool,
) -> CursorSidecarSpec {
    let setting_sources = if is_text_generation {
        Vec::new()
    } else if let Some(raw) = env.get("CURSOR_AGENT_SETTING_SOURCES") {
        raw.split(',')
            .map(str::trim)
            .filter(|s| matches!(*s, "user" | "project" | "local"))
            .map(ToOwned::to_owned)
            .collect()
    } else {
        vec!["user".to_owned(), "project".to_owned()]
    };

    let mut overrides = BTreeMap::new();
    overrides.insert("CURSOR_STATE_ROOT".to_owned(), state_root.to_owned());
    CursorSidecarSpec {
        executable: executable.to_owned(),
        state_root: state_root.to_owned(),
        setting_sources,
        use_http1_for_agent,
        env: build_child_env(env, &[], &overrides),
    }
}

/// Core local CLI provider driver slugs probed during workspace/machine discovery.
pub const DISCOVERY_CLI_DRIVERS: &[&str] = &[
    "codex",
    "claudeAgent",
    "cursor",
    "opencode",
    "antigravity",
    "pi",
    "kiro",
    "fx",
    "acpRegistry",
];

/// Normalizes common driver slug aliases (e.g. `"claude"` -> `"claudeAgent"`, `"museCode"` -> `"muse"`, `"replay-dev"` -> `"devReplay"`).
#[must_use]
pub fn normalize_driver_slug(raw: &str) -> &str {
    let trimmed = raw.trim();
    match trimmed.to_ascii_lowercase().as_str() {
        "codex" => "codex",
        "claude" | "claude-code" | "claude_agent" | "claudeagent" => "claudeAgent",
        "cursor" | "cursor-agent" => "cursor",
        "grok" => "grok",
        "opencode" | "open-code" => "opencode",
        "antigravity" | "agy" => "antigravity",
        "kiro" | "kiro-cli" => "kiro",
        "pi" => "pi",
        "muse" | "musecode" | "muse-code" => "muse",
        "fx" => "fx",
        "acp" | "acpregistry" | "acp-registry" => "acpRegistry",
        "replay" | "replay-dev" | "dev-replay" | "dev_replay" | "devreplay" => "devReplay",
        _ => trimmed,
    }
}

/// Looks up a built-in [`ProviderDriverSpec`] by canonical slug or recognized alias.
#[must_use]
pub fn find_builtin_driver_spec(driver_or_alias: &str) -> Option<&'static ProviderDriverSpec> {
    let canonical = normalize_driver_slug(driver_or_alias);
    BUILTIN_DRIVER_SPECS
        .iter()
        .find(|spec| spec.slug == canonical)
}

/// Builds a fast, non-interactive `--version` [`super::ProviderProbeSpec`] for any CLI-backed driver.
///
/// Returns `None` for unknown slugs or built-in in-process drivers (`"devReplay"`).
#[must_use]
pub fn builtin_cli_probe_spec(
    env: &SanitizedEnv,
    driver_or_alias: &str,
) -> Option<super::ProviderProbeSpec> {
    let spec = find_builtin_driver_spec(driver_or_alias)?;
    if spec.slug == "devReplay" {
        return None;
    }
    if spec.slug == "pi" {
        return Some(pi_probe_spec(env, spec.default_binary));
    }
    let removed = if spec.slug == "claudeAgent" {
        CLAUDE_HOST_SESSION_VARIABLES
    } else {
        &[][..]
    };
    let mut overrides = BTreeMap::new();
    overrides.insert("NO_COLOR".to_owned(), "1".to_owned());
    Some(super::ProviderProbeSpec {
        executable: spec.default_binary.to_owned(),
        args: vec!["--version".to_owned()],
        env: build_child_env(env, removed, &overrides),
        removed_env_keys: removed.iter().map(|s| (*s).to_owned()).collect(),
    })
}
