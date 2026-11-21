//! Grok CLI ACP driver: maintenance routing (`BUG-13`/`BUG-14`),
//! `ctrl_c` cancellation metadata, model parsing, and slash command filtering.

use std::collections::BTreeMap;

use serde_json::{Value, json};
use t3_protocol::provider_env::{
    MaintenanceCapabilities, MaintenanceResolutionInput, ProviderMaintenanceDefinition,
    UpdateCommandSpec, resolve_provider_maintenance,
};

use crate::core::CatalogModelEntry;

use super::{ProviderLaunchSpec, build_child_env};

/// Published npm package identifier for the Grok CLI.
pub const GROK_NPM_PACKAGE: &str = "@xai-official/grok";
/// OAuth referrer passed when spawning Grok CLI sessions from T3 Code.
pub const GROK_OAUTH2_REFERRER: &str = "t3code";
/// ACP `_meta.cancelTrigger` marker expected by Grok CLI on cancellation.
pub const GROK_ACP_CANCEL_TRIGGER: &str = "ctrl_c";

/// Resolved Grok CLI self-update plan (`BUG-13` / `BUG-14`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrokUpdatePlan {
    /// Underlying resolved maintenance capabilities from `t3-protocol`.
    pub capabilities: MaintenanceCapabilities,
    /// Concrete command to run when updating, after package-manager vs native fallback routing.
    pub command: UpdateCommandSpec,
    /// Extra environment variables injected when executing the update command
    /// (for example `npm_config_allow_scripts=@xai-official/grok`).
    pub env_overrides: BTreeMap<String, String>,
    /// Whether the plan fell back to native `grok update`.
    pub used_native_fallback: bool,
}

/// Resolves Grok CLI update execution through [`resolve_provider_maintenance`]
/// before falling back to native `grok update` (`BUG-13` & `BUG-14`).
///
/// Ensures package-manager installs (Homebrew, mise, npm, pnpm, bun, yarn)
/// take precedence over bare binary invocation and sets
/// `npm_config_allow_scripts=@xai-official/grok` when npm handles the install.
#[must_use]
pub fn resolve_grok_maintenance(
    input: &MaintenanceResolutionInput,
    native_binary: Option<&str>,
) -> GrokUpdatePlan {
    let def = ProviderMaintenanceDefinition::grok();
    let capabilities = resolve_provider_maintenance(&def, input);

    let binary = native_binary
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("grok");

    if let Some(spec) = capabilities.update_command.clone() {
        let mut env_overrides = BTreeMap::new();
        inject_npm_allow_scripts(&spec, &mut env_overrides);
        let exe_name = spec
            .executable
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(&spec.executable);
        let used_native_fallback = (exe_name.eq_ignore_ascii_case("grok")
            || exe_name.eq_ignore_ascii_case("grok.exe")
            || spec.executable == binary)
            && spec.args == ["update"];
        return GrokUpdatePlan {
            capabilities,
            command: spec,
            env_overrides,
            used_native_fallback,
        };
    }

    GrokUpdatePlan {
        capabilities,
        command: UpdateCommandSpec {
            executable: binary.to_owned(),
            args: vec!["update".to_owned()],
        },
        env_overrides: BTreeMap::new(),
        used_native_fallback: true,
    }
}

fn inject_npm_allow_scripts(spec: &UpdateCommandSpec, env: &mut BTreeMap<String, String>) {
    let bin = spec
        .executable
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(&spec.executable);
    let is_npm = bin.eq_ignore_ascii_case("npm") || bin.eq_ignore_ascii_case("npm.cmd");
    if is_npm {
        env.insert(
            "npm_config_allow_scripts".to_owned(),
            GROK_NPM_PACKAGE.to_owned(),
        );
    }
}

/// Builds a sanitized Grok ACP launch specification (`BUG-12`).
///
/// Sets `GROK_OAUTH2_REFERRER=t3code` and appends `--model <model>` when given.
///
/// # Examples
///
/// ```
/// use t3_protocol::provider_env::SanitizedEnv;
/// use t3_providers::drivers::build_grok_launch_spec;
///
/// let env = SanitizedEnv::from_entries([("PATH", "/usr/bin")], false);
/// let spec = build_grok_launch_spec(&env, Some("/repo"), "grok", Some("grok-4"));
/// assert_eq!(spec.args, vec!["acp", "--model", "grok-4"]);
/// assert_eq!(spec.env.get("GROK_OAUTH2_REFERRER").map(String::as_str), Some("t3code"));
/// assert_eq!(spec.cancel_behavior, "ctrl_c");
/// ```
#[must_use]
pub fn build_grok_launch_spec(
    env: &t3_protocol::provider_env::SanitizedEnv,
    cwd: Option<&str>,
    binary_path: &str,
    model: Option<&str>,
) -> ProviderLaunchSpec {
    let mut args = vec!["acp".to_owned()];
    if let Some(m) = model.map(str::trim).filter(|s| !s.is_empty()) {
        args.push("--model".to_owned());
        args.push(m.to_owned());
    }
    let mut overrides = BTreeMap::new();
    overrides.insert(
        "GROK_OAUTH2_REFERRER".to_owned(),
        GROK_OAUTH2_REFERRER.to_owned(),
    );

    ProviderLaunchSpec {
        executable: binary_path.to_owned(),
        args,
        env: build_child_env(env, &[], &overrides),
        removed_env_keys: Vec::new(),
        cwd: cwd.map(ToOwned::to_owned),
        cancel_behavior: GROK_ACP_CANCEL_TRIGGER,
    }
}

/// Returns Grok ACP `initialize._meta` advertising extension client capabilities.
#[must_use]
pub fn grok_acp_initialize_meta() -> Value {
    json!({ "clientType": "extension" })
}

/// Returns Grok ACP `session/cancel._meta` (`{ "cancelTrigger": "ctrl_c" }`).
#[must_use]
pub fn grok_acp_cancel_meta() -> BTreeMap<String, Value> {
    let mut meta = BTreeMap::new();
    meta.insert(
        "cancelTrigger".to_owned(),
        Value::String(GROK_ACP_CANCEL_TRIGGER.to_owned()),
    );
    meta
}

/// Extracts Grok slash commands advertised on `initialize._meta`, filtering out
/// interactive-CLI-only commands (`always-approve`, `context`).
#[must_use]
pub fn grok_slash_commands_from_initialize(meta: Option<&Value>) -> Vec<String> {
    let Some(list) = meta
        .and_then(|m| m.get("slashCommands").or_else(|| m.get("commands")))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for item in list {
        let name = item
            .as_str()
            .or_else(|| item.get("name").and_then(Value::as_str))
            .map(|s| s.trim().trim_start_matches('/'));
        if let Some(cmd) = name
            && !cmd.is_empty()
            && !matches!(cmd, "always-approve" | "context")
            && !out.iter().any(|seen: &String| seen == cmd)
        {
            out.push(cmd.to_owned());
        }
    }
    out
}

/// Parses `grok models` stdout lines into deduplicated [`CatalogModelEntry`] values.
#[must_use]
pub fn parse_grok_models_cli_output(stdout: &str) -> Vec<CatalogModelEntry> {
    let mut seen = Vec::<CatalogModelEntry>::new();
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with('#')
            || trimmed.eq_ignore_ascii_case("available models:")
            || trimmed.starts_with("---")
        {
            continue;
        }
        let cleaned = trimmed.trim_start_matches(['*', '-', '•', ' ']).trim();
        let token = cleaned.split([' ', '\t', '(']).next().unwrap_or("").trim();
        if token.is_empty()
            || !token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'/'))
        {
            continue;
        }
        if seen.iter().any(|m| m.slug == token) {
            continue;
        }
        let lower = token.to_ascii_lowercase();
        seen.push(CatalogModelEntry {
            slug: token.to_owned(),
            name: token.to_owned(),
            is_custom: false,
            supports_reasoning: lower.contains("reasoning") || lower.contains("grok-4"),
        });
    }
    seen
}
