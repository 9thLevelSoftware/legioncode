//! Dynamic provider driver resolution from `settings.json`, `SanitizedEnv`, and `PATH`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use legion_contracts::orchestration::Run;
use legion_contracts::policy::RuntimeMode;
use legion_protocol::provider_env::SanitizedEnv;
use legion_providers::core::resolve_cli_model_arg;
use legion_providers::drivers::claude_lifecycle::{
    ClaudeHistoryEntry, claude_resume_interrupted_turn_env,
};
use legion_providers::drivers::{
    AntigravityInstallTarget, CLAUDE_HOST_SESSION_VARIABLES, ProviderLaunchSpec,
    build_antigravity_launch_spec, build_child_env, build_claude_env, build_grok_launch_spec,
    codex_app_server_args, find_executable_on_path, fx_launch_spec, kiro_launch_spec,
    normalize_driver_slug, pi_launch_args, resolve_antigravity_instance_directories,
    resolve_claude_executable, resolve_cli_probe_command,
};

use crate::acp_registry::AcpDriverSpec;
use crate::error::ServerRuntimeError;
use crate::outbox_turn_dispatch::ProviderDriverMode;

#[path = "driver_resolver_config.rs"]
mod config;
use config::{InstanceConfig, read_instance_config};

/// Resolves a launch executable and arguments into OS-spawnable `(program, args)`.
///
/// Wraps Windows `.cmd`/`.bat` shims via `cmd.exe /D /C` and resolves bare command
/// names on `PATH`, while preserving direct file paths to non-batch binaries.
#[must_use]
pub(crate) fn resolve_spawn_program_args(
    executable: &str,
    args: &[String],
) -> (String, Vec<String>) {
    let trimmed = executable.trim();
    let path = Path::new(trimmed);
    let is_windows_shim = cfg!(windows)
        && path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"));
    if (path.is_absolute() || path.components().count() > 1) && path.is_file() && !is_windows_shim {
        return (trimmed.to_owned(), args.to_vec());
    }
    let san = SanitizedEnv::from_entries(std::env::vars(), cfg!(windows));
    resolve_cli_probe_command(
        trimmed,
        args,
        san.get("PATH"),
        san.get("PATHEXT"),
        cfg!(windows),
    )
    .unwrap_or_else(|| (trimmed.to_owned(), args.to_vec()))
}

fn resolve_binary(
    configured: Option<&str>,
    default_bin: &str,
    san: &SanitizedEnv,
    driver: &str,
) -> Result<PathBuf, ServerRuntimeError> {
    if let Some(raw) = configured.map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(found) =
            find_executable_on_path(raw, san.get("PATH"), san.get("PATHEXT"), cfg!(windows))
        {
            return Ok(found);
        }
        let direct = Path::new(raw);
        if direct.is_file() {
            return Ok(direct.to_path_buf());
        }
        return Err(ServerRuntimeError::NotFound(format!(
            "configured `{driver}` binary `{raw}` was not found on disk or PATH"
        )));
    }
    find_executable_on_path(
        default_bin,
        san.get("PATH"),
        san.get("PATHEXT"),
        cfg!(windows),
    )
    .ok_or_else(|| {
        ServerRuntimeError::NotFound(format!(
            "`{driver}` CLI (`{default_bin}`) is not installed or not on PATH"
        ))
    })
}

fn claude_permission_mode(mode: RuntimeMode) -> &'static str {
    match mode {
        RuntimeMode::FullAccess => "bypassPermissions",
        RuntimeMode::AutoAcceptEdits => "acceptEdits",
        RuntimeMode::Auto => "auto",
        RuntimeMode::ApprovalRequired => "default",
    }
}

fn build_claude_mode(
    cfg: InstanceConfig,
    run: &Run,
    runtime_mode: RuntimeMode,
    san: &SanitizedEnv,
) -> Result<ProviderDriverMode, ServerRuntimeError> {
    let found = resolve_binary(cfg.binary_path.as_deref(), "claude", san, "claudeAgent")?;
    let found_str = found.to_string_lossy().into_owned();
    let resolved = resolve_claude_executable(&found_str, cfg!(windows), |p| Path::new(p).is_file())
        .unwrap_or(found_str);
    let mut executable = resolved.clone();
    let mut args = Vec::new();
    let is_script = Path::new(&resolved)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "js" | "mjs" | "cjs"));
    if is_script {
        let node = resolve_binary(None, "node", san, "claudeAgent")?;
        executable = node.to_string_lossy().into_owned();
        args.push(resolved);
    }
    args.extend([
        "-p".to_owned(),
        "--output-format".to_owned(),
        "stream-json".to_owned(),
        "--verbose".to_owned(),
        "--input-format".to_owned(),
        "stream-json".to_owned(),
        "--include-partial-messages".to_owned(),
        "--permission-prompt-tool=stdio".to_owned(),
        "--permission-mode".to_owned(),
        claude_permission_mode(runtime_mode).to_owned(),
    ]);
    if let Some(model) = resolve_cli_model_arg("claudeAgent", &run.model_selection.model) {
        args.push("--model".to_owned());
        args.push(model);
    }
    args.extend(cfg.extra_args);
    let mut env = build_claude_env(san, cfg.home_path.as_deref());
    env.extend(cfg.env);
    Ok(ProviderDriverMode::ClaudeStreamJson(ProviderLaunchSpec {
        executable,
        args,
        env,
        removed_env_keys: CLAUDE_HOST_SESSION_VARIABLES
            .iter()
            .map(|k| (*k).to_owned())
            .collect(),
        cwd: None,
        cancel_behavior: "interrupt",
    }))
}

fn acp_spec(
    program: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    driver: &str,
) -> AcpDriverSpec {
    let mut spec = AcpDriverSpec::new(program)
        .with_args(args)
        .with_driver(driver);
    spec.env_overrides = env;
    spec
}

/// Resolves a `ProviderDriverMode` dynamically from `settings.json` + sanitized `PATH`.
pub(crate) fn resolve_dynamic_driver(
    instance_id: &str,
    run: &Run,
    runtime_mode: RuntimeMode,
    settings_path: Option<&Path>,
) -> Result<ProviderDriverMode, ServerRuntimeError> {
    if normalize_driver_slug(instance_id) == "devReplay" {
        return Ok(ProviderDriverMode::DevReplay);
    }
    let cfg = read_instance_config(settings_path, instance_id);
    let san = SanitizedEnv::from_entries(std::env::vars(), cfg!(windows));
    let driver_slug = cfg.driver.clone();
    match driver_slug.as_str() {
        "devReplay" => Ok(ProviderDriverMode::DevReplay),
        "claudeAgent" => build_claude_mode(cfg, run, runtime_mode, &san),
        "codex" => {
            let bin = resolve_binary(cfg.binary_path.as_deref(), "codex", &san, "codex")?;
            let mut env = build_child_env(&san, &[], &cfg.env);
            if let Some(home) = cfg.home_path {
                env.insert("CODEX_HOME".to_owned(), home);
            }
            let mut args = codex_app_server_args(&[]);
            args.extend(cfg.extra_args);
            Ok(ProviderDriverMode::CodexAppServer(ProviderLaunchSpec {
                executable: bin.to_string_lossy().into_owned(),
                args,
                env,
                removed_env_keys: Vec::new(),
                cwd: None,
                cancel_behavior: "interrupt",
            }))
        }
        "cursor" => {
            let bin = resolve_binary(cfg.binary_path.as_deref(), "cursor-agent", &san, "cursor")?;
            Ok(ProviderDriverMode::CursorSidecar(ProviderLaunchSpec {
                executable: bin.to_string_lossy().into_owned(),
                args: cfg.extra_args,
                env: build_child_env(&san, &[], &cfg.env),
                removed_env_keys: Vec::new(),
                cwd: None,
                cancel_behavior: "interrupt",
            }))
        }
        "pi" => {
            let bin = resolve_binary(cfg.binary_path.as_deref(), "pi", &san, "pi")?;
            let model = resolve_cli_model_arg("pi", &run.model_selection.model);
            let args = pi_launch_args(false, model.as_deref(), None, &cfg.extra_args)
                .map_err(|e| ServerRuntimeError::BadRequest(e.to_string()))?;
            Ok(ProviderDriverMode::PiCli(ProviderLaunchSpec {
                executable: bin.to_string_lossy().into_owned(),
                args,
                env: build_child_env(&san, &[], &cfg.env),
                removed_env_keys: Vec::new(),
                cwd: None,
                cancel_behavior: "interrupt",
            }))
        }
        _ => resolve_acp_and_http_driver(
            &driver_slug,
            instance_id,
            cfg,
            run,
            runtime_mode,
            &san,
            settings_path,
        ),
    }
}

fn resolve_acp_and_http_driver(
    driver_slug: &str,
    instance_id: &str,
    cfg: InstanceConfig,
    run: &Run,
    runtime_mode: RuntimeMode,
    san: &SanitizedEnv,
    settings_path: Option<&Path>,
) -> Result<ProviderDriverMode, ServerRuntimeError> {
    match driver_slug {
        "antigravity" => {
            let bin = resolve_binary(cfg.binary_path.as_deref(), "agy", san, "antigravity")?;
            let root = settings_path
                .and_then(Path::parent)
                .map_or_else(|| ".".to_owned(), |p| p.to_string_lossy().into_owned());
            let tmp = std::env::temp_dir().to_string_lossy().into_owned();
            let dirs = resolve_antigravity_instance_directories(&root, &tmp, instance_id);
            let target = AntigravityInstallTarget::AgyCli {
                executable: bin.to_string_lossy().into_owned(),
            };
            let mut launch = build_antigravity_launch_spec(san, &target, &dirs, None);
            launch.args.extend(cfg.extra_args);
            launch.env.extend(cfg.env);
            Ok(ProviderDriverMode::AcpCli(acp_spec(
                launch.executable,
                launch.args,
                launch.env,
                "antigravity",
            )))
        }
        "kiro" => {
            let bin = resolve_binary(cfg.binary_path.as_deref(), "kiro-cli", san, "kiro")?;
            let mut spec =
                kiro_launch_spec(san, &bin.to_string_lossy(), None, runtime_mode, false, None);
            spec.launch.args.extend(cfg.extra_args);
            spec.launch.env.extend(cfg.env);
            Ok(ProviderDriverMode::AcpCli(acp_spec(
                spec.launch.executable,
                spec.launch.args,
                spec.launch.env,
                "kiro",
            )))
        }
        "fx" => {
            let bin = resolve_binary(cfg.binary_path.as_deref(), "fx", san, "fx")?;
            let mut spec = fx_launch_spec(san, &bin.to_string_lossy(), None, runtime_mode);
            spec.launch.args.extend(cfg.extra_args);
            spec.launch.env.extend(cfg.env);
            Ok(ProviderDriverMode::AcpCli(acp_spec(
                spec.launch.executable,
                spec.launch.args,
                spec.launch.env,
                "fx",
            )))
        }
        "opencode" | "grok" | "acpRegistry" => {
            resolve_http_or_registry_driver(driver_slug, instance_id, cfg, run, san)
        }
        other => Err(ServerRuntimeError::NotFound(format!(
            "no runtime provider driver registered for `{other}` (`{instance_id}`)"
        ))),
    }
}

fn resolve_http_or_registry_driver(
    driver_slug: &str,
    instance_id: &str,
    cfg: InstanceConfig,
    run: &Run,
    san: &SanitizedEnv,
) -> Result<ProviderDriverMode, ServerRuntimeError> {
    match driver_slug {
        "opencode" => {
            if let Some(url) = cfg.base_url {
                return Ok(ProviderDriverMode::OpenCodeHttp {
                    base_url: url,
                    auth_header: None,
                    model: resolve_cli_model_arg("opencode", &run.model_selection.model),
                });
            }
            let bin = resolve_binary(cfg.binary_path.as_deref(), "opencode", san, "opencode")?;
            let mut args = vec!["acp".to_owned()];
            args.extend(cfg.extra_args);
            Ok(ProviderDriverMode::AcpCli(acp_spec(
                bin.to_string_lossy().into_owned(),
                args,
                cfg.env,
                "opencode",
            )))
        }
        "grok" => {
            if let Some(key) = cfg
                .api_key
                .or_else(|| san.get("XAI_API_KEY").map(ToOwned::to_owned))
            {
                return Ok(ProviderDriverMode::GrokHttp {
                    endpoint_url: cfg.base_url.unwrap_or_default(),
                    api_key: key,
                    model: resolve_cli_model_arg("grok", &run.model_selection.model),
                });
            }
            let bin = resolve_binary(cfg.binary_path.as_deref(), "grok", san, "grok")?;
            let model = resolve_cli_model_arg("grok", &run.model_selection.model);
            let mut launch =
                build_grok_launch_spec(san, None, &bin.to_string_lossy(), model.as_deref());
            launch.args.extend(cfg.extra_args);
            launch.env.extend(cfg.env);
            Ok(ProviderDriverMode::AcpCli(acp_spec(
                launch.executable,
                launch.args,
                launch.env,
                "grok",
            )))
        }
        "acpRegistry" => {
            let bin = cfg.binary_path.ok_or_else(|| {
                ServerRuntimeError::NotFound(format!(
                    "no binary path configured for ACP Registry instance `{instance_id}`"
                ))
            })?;
            let resolved = resolve_binary(Some(&bin), &bin, san, "acpRegistry")?;
            Ok(ProviderDriverMode::AcpCli(acp_spec(
                resolved.to_string_lossy().into_owned(),
                cfg.extra_args,
                cfg.env,
                "acpRegistry",
            )))
        }
        other => Err(ServerRuntimeError::NotFound(format!(
            "no runtime provider driver registered for `{other}` (`{instance_id}`)"
        ))),
    }
}

/// Applies interrupted-turn resume environment variables for Claude only when the persisted
/// history does not end on a failed `<synthetic>` placeholder (`#41120`).
pub fn apply_claude_resume_env(
    env: &mut BTreeMap<String, String>,
    is_resume: bool,
    history: &[ClaudeHistoryEntry],
) {
    if let Some(pairs) = claude_resume_interrupted_turn_env(is_resume, history) {
        for (key, value) in pairs {
            env.insert(key.to_owned(), value.to_owned());
        }
    }
}

#[cfg(test)]
#[path = "driver_resolver_tests.rs"]
mod tests;
