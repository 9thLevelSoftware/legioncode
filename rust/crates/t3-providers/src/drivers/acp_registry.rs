//! ACP Registry local binary flavor detection (`droid`, `glm`, `vibe`),
//! missing-upstream archive refresh checks, and `DeepSeek` CLI probe history isolation.

use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;
use serde_json::json;

/// Normalized ACP Registry adapter capabilities flavor (`84eb429049`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpRegistryLocalFlavor {
    /// Factory Droid (`factory-droid`, `droid`, `factory-droid.exe`).
    Droid,
    /// GLM ACP agent (`glm-acp-agent`, `glm`, `glm-acp-agent.cmd`).
    Glm,
    /// Mistral Vibe (`mistral-vibe`, `vibe`, `vibe-acp`).
    Vibe,
    /// Standard generic ACP registry agent.
    Standard,
}

fn droid_regex() -> Option<&'static Regex> {
    static RE: OnceLock<Option<Regex>> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)^(?:factory-)?droid(?:\.exe|\.cmd)?$").ok())
        .as_ref()
}

fn glm_regex() -> Option<&'static Regex> {
    static RE: OnceLock<Option<Regex>> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)^glm(?:-acp-agent)?(?:\.exe|\.cmd)?$").ok())
        .as_ref()
}

fn vibe_regex() -> Option<&'static Regex> {
    static RE: OnceLock<Option<Regex>> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)^(?:mistral-)?vibe(?:-acp)?(?:\.exe|\.cmd)?$").ok())
        .as_ref()
}

fn dsh_regex() -> Option<&'static Regex> {
    static RE: OnceLock<Option<Regex>> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)^dsh(?:\.cmd|\.exe)?$").ok())
        .as_ref()
}

/// Identifies `droid`, `glm`, or `vibe` even when configured as a local `commandPath` override (`84eb429049`).
#[must_use]
pub fn detect_acp_registry_local_flavor(
    registry_agent_id: &str,
    is_local: bool,
    command_path: &str,
) -> AcpRegistryLocalFlavor {
    let exe = command_path.rsplit(['/', '\\']).next().unwrap_or_default();
    if registry_agent_id == "factory-droid"
        || (is_local && droid_regex().is_some_and(|re| re.is_match(exe)))
    {
        return AcpRegistryLocalFlavor::Droid;
    }
    if registry_agent_id == "glm-acp-agent"
        || (is_local && glm_regex().is_some_and(|re| re.is_match(exe)))
    {
        return AcpRegistryLocalFlavor::Glm;
    }
    if registry_agent_id == "mistral-vibe"
        || (is_local && vibe_regex().is_some_and(|re| re.is_match(exe)))
    {
        return AcpRegistryLocalFlavor::Vibe;
    }
    AcpRegistryLocalFlavor::Standard
}

impl AcpRegistryLocalFlavor {
    /// Returns the prompt failure mapping agent ID (`mistral-vibe` for Vibe) (`84eb429049`).
    #[must_use]
    pub fn prompt_failure_agent_id(self, registry_agent_id: &str) -> &str {
        if self == Self::Vibe {
            "mistral-vibe"
        } else {
            registry_agent_id
        }
    }

    /// Returns `true` when per-turn ACP prompt usage counters are verified live (`glm-acp-agent`).
    #[must_use]
    pub const fn reports_per_turn_prompt_usage(self) -> bool {
        matches!(self, Self::Glm)
    }

    /// Returns `true` when Mistral Vibe ACP extensions must be registered.
    #[must_use]
    pub const fn registers_vibe_extensions(self) -> bool {
        matches!(self, Self::Vibe)
    }

    /// Returns `true` when Droid elicitation forms apply.
    #[must_use]
    pub const fn uses_droid_elicitation_form(self) -> bool {
        matches!(self, Self::Droid)
    }
}

/// Returns `true` when a cached ACP registry archive failure (`download_failed`) points to an
/// outdated pinned version and retrying against a freshly fetched `refreshed_version` should run (`714997c420`).
#[must_use]
pub fn acp_registry_should_retry_with_refreshed_index(
    error_reason: &str,
    cached_version: Option<&str>,
    refreshed_version: Option<&str>,
) -> bool {
    error_reason == "download_failed" && refreshed_version != cached_version
}

/// Generated `--patch` JSON and rewritten launch args keeping `DeepSeek` ACP probes out of
/// user session history (`31e31c608d`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeepSeekProbeIsolation {
    /// JSON payload to write into `<temp_dir>/probe.json`.
    pub patch_json: String,
    /// Full path passed to `--patch <patch_path>`.
    pub patch_path: String,
    /// Rewritten argument list preserving trailing arguments after `--`.
    pub modified_args: Vec<String>,
}

/// Detects `DeepSeek` Harness (`dsh`) running with its `acp` profile and generates
/// temporary `--patch` overrides disabling persistent cache and isolating sessions (`31e31c608d`).
#[must_use]
pub fn build_deepseek_probe_history_isolation(
    command_path: &str,
    args: &[String],
    temp_probe_dir: &Path,
) -> Option<DeepSeekProbeIsolation> {
    let executable = command_path.rsplit(['/', '\\']).next().unwrap_or_default();
    if !dsh_regex().is_some_and(|re| re.is_match(executable)) {
        return None;
    }

    let separator = args.iter().position(|a| a == "--").unwrap_or(args.len());
    let launcher_args = &args[..separator];
    let uses_acp_profile = launcher_args.first().map(String::as_str) == Some("acp")
        || launcher_args.iter().any(|a| a == "--profile=acp")
        || launcher_args
            .windows(2)
            .any(|pair| pair[0] == "--profile" && pair[1] == "acp");
    if !uses_acp_profile {
        return None;
    }

    let patch_path_buf = temp_probe_dir.join("probe.json");
    let sessions_root = temp_probe_dir.join("sessions");
    let patch_json = json!([
        {
            "id": "session-persistence-jsonl",
            "config": { "root": sessions_root.to_string_lossy() }
        },
        {
            "id": "session-projection-cache",
            "disabled": true
        }
    ])
    .to_string();
    let patch_path = patch_path_buf.to_string_lossy().into_owned();

    let mut modified_args = launcher_args.to_vec();
    modified_args.push("--patch".to_owned());
    modified_args.push(patch_path.clone());
    modified_args.extend_from_slice(&args[separator..]);

    Some(DeepSeekProbeIsolation {
        patch_json,
        patch_path,
        modified_args,
    })
}
