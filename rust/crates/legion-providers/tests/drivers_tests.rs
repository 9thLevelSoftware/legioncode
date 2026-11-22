//! Integration tests for provider driver specifications (`BUG-11`..`BUG-14`),
//! built-in CLIs, and deterministic `Replay (dev)` scenario generation.

use legion_contracts::policy::RuntimeMode;
use legion_protocol::provider_env::{MaintenanceResolutionInput, SanitizedEnv};
use legion_providers::acp::{AcpStdioCodec, ToolCallStatus};
use legion_providers::core::{
    ModelCatalog, ServerModelEntry, UsageLimitsSnapshot, UsageWindow, UsageWindowKind,
};
use legion_providers::drivers::{
    ANTIGRAVITY_FORCE_FILE_STORAGE_ENV, AntigravityAuthConfig, AntigravityInstallTarget,
    AntigravityProbeOutcome, AntigravityVersionLeases, BUILTIN_DRIVER_SPECS,
    FX_ACP_CANCEL_BEHAVIOR, ProviderProbeStatus, ProviderSnapshotState,
    build_antigravity_launch_spec, build_claude_env, build_grok_launch_spec, codex_app_server_args,
    codex_text_gen_args, codex_thread_start_envelope, codex_turn_start_envelope,
    cursor_isolated_sidecar_spec, fill_compaction_recents, fx_launch_spec,
    fx_tool_call_was_cancelled, grok_acp_cancel_meta, grok_acp_initialize_meta,
    grok_slash_commands_from_initialize, kiro_launch_spec, muse_launch_spec, muse_verbatim_path,
    normalize_registry_command_path, opencode_basic_auth_header, opencode_server_env,
    parse_grok_models_cli_output, pi_launch_args, pi_probe_spec, probe_antigravity,
    reconcile_antigravity_snapshot, resolve_antigravity_instance_directories,
    resolve_claude_executable, resolve_grok_maintenance, validate_registry_executable,
};
use serde_json::json;
use tempfile::tempdir;

#[test]
fn antigravity_bug11_install_checked_before_auth_and_clears_stale_cache() {
    let broken_auth = AntigravityAuthConfig {
        has_api_key: false,
        has_oauth_token: false,
        validation_error: Some("Missing GEMINI_API_KEY".to_owned()),
    };
    let outcome = probe_antigravity(None, &broken_auth, "work", "/var/legioncode", "/tmp");
    assert!(matches!(
        outcome,
        AntigravityProbeOutcome::NotInstalled { .. }
    ));

    let stale = ProviderSnapshotState {
        installed: true,
        status: ProviderProbeStatus::Ready,
        auth_configured: true,
        models: vec![ServerModelEntry {
            slug: "gemini-2.5-pro".to_owned(),
            name: "Gemini 2.5 Pro".to_owned(),
            short_name: "Gemini 2.5 Pro".to_owned(),
            is_custom: false,
            supports_reasoning: true,
        }],
        slash_commands: vec!["help".to_owned()],
        skills: vec!["refactor".to_owned()],
        usage_limits: Some(UsageLimitsSnapshot {
            windows: vec![UsageWindow {
                id: "sess".to_owned(),
                label: "Session".to_owned(),
                kind: UsageWindowKind::Session,
                used_percent: 42.0,
                resets_at: None,
            }],
            updated_at: Some("2026-11-20T00:00:00Z".to_owned()),
        }),
        supports_text_generation: true,
        message: None,
    };

    let reconciled = reconcile_antigravity_snapshot(stale, &outcome);
    assert!(!reconciled.installed);
    assert_eq!(reconciled.status, ProviderProbeStatus::NotInstalled);
    assert_eq!(reconciled.models.len(), 0);
    assert_eq!(reconciled.slash_commands.len(), 0);
    assert_eq!(reconciled.skills.len(), 0);
    assert!(reconciled.usage_limits.is_none());
    assert!(!reconciled.supports_text_generation);
}

#[test]
fn antigravity_sha256_env_isolation_and_version_lease_refcounting() {
    let dirs_a = resolve_antigravity_instance_directories("/data/", "/tmp/", "antigravity:a");
    let dirs_b = resolve_antigravity_instance_directories("/data/", "/tmp/", "antigravity:b");
    assert_ne!(dirs_a.gemini_home, dirs_b.gemini_home);
    assert_ne!(dirs_a.runtime_temp, dirs_b.runtime_temp);
    assert_eq!(dirs_a.instance_digest_hex.len(), 64);

    let env = SanitizedEnv::from_entries(
        [
            ("PATH", "/usr/bin"),
            ("GEMINI_API_KEY", "leak-host-key"),
            ("LEGIONCODE_TOKEN", "blocked-token"),
        ],
        false,
    );
    let target = AntigravityInstallTarget::AcpServer {
        executable: "/opt/agy/agy_acp_server".to_owned(),
        localharness_external: "/opt/agy/localharness_external".to_owned(),
    };
    let launch = build_antigravity_launch_spec(&env, &target, &dirs_a, Some("/repo"));
    assert!(!launch.env.contains_key("GEMINI_API_KEY"));
    assert!(!launch.env.contains_key("LEGIONCODE_TOKEN"));
    assert_eq!(
        launch.env.get("GEMINI_HOME").map(String::as_str),
        Some(dirs_a.gemini_home.as_str())
    );
    assert_eq!(
        launch
            .env
            .get(ANTIGRAVITY_FORCE_FILE_STORAGE_ENV)
            .map(String::as_str),
        Some("1")
    );

    let leases = AntigravityVersionLeases::new();
    let g1 = leases.acquire("1.2.3");
    let g2 = leases.acquire("1.2.3");
    assert_eq!(leases.lease_count("1.2.3"), 2);
    assert!(leases.is_leased("1.2.3"));
    let installed = vec!["1.2.2".to_owned(), "1.2.3".to_owned(), "1.2.4".to_owned()];
    assert_eq!(
        leases.prunable_versions(&installed, Some("1.2.4")),
        vec!["1.2.2".to_owned()]
    );
    drop(g1);
    assert!(leases.is_leased("1.2.3"));
    g2.release();
    assert!(!leases.is_leased("1.2.3"));
    assert_eq!(
        leases.prunable_versions(&installed, Some("1.2.4")),
        vec!["1.2.2".to_owned(), "1.2.3".to_owned()]
    );
}

#[test]
fn grok_maintenance_routing_and_acp_metadata() {
    let input = MaintenanceResolutionInput {
        resolved_binary_path: Some("/usr/local/bin/grok".to_owned()),
        ..Default::default()
    };
    let plan = resolve_grok_maintenance(&input, Some("/usr/local/bin/grok"));
    assert_ne!(plan.command.executable, "");

    let env = SanitizedEnv::from_entries(
        [
            ("PATH", "/usr/bin"),
            ("APPIMAGE", "/tmp/legioncode.AppImage"),
            ("LEGIONCODE_DEV_AUTH_TOKEN", "dev-secret"),
        ],
        false,
    );
    let spec = build_grok_launch_spec(&env, Some("/repo"), "grok", Some("grok-4"));
    assert!(!spec.env.contains_key("APPIMAGE"));
    assert!(!spec.env.contains_key("LEGIONCODE_DEV_AUTH_TOKEN"));
    assert_eq!(spec.args, vec!["acp", "--model", "grok-4"]);
    assert_eq!(
        grok_acp_initialize_meta()
            .get("clientType")
            .and_then(|v| v.as_str()),
        Some("extension")
    );
    assert_eq!(
        grok_acp_cancel_meta()
            .get("cancelTrigger")
            .and_then(|v| v.as_str()),
        Some("ctrl_c")
    );

    let cmds = grok_slash_commands_from_initialize(Some(&json!({
        "slashCommands": ["compact", "always-approve", "context", "/review", "compact"]
    })));
    assert_eq!(cmds, vec!["compact", "review"]);

    let models = parse_grok_models_cli_output(
        "Available models:\n* grok-4-fast-reasoning (default)\n- grok-code-fast-1\n",
    );
    let catalog = ModelCatalog::apply(models, Vec::new(), None);
    assert_eq!(catalog.models.len(), 2);
    assert!(catalog.models[0].supports_reasoning);
}

#[test]
fn builtin_drivers_cover_codex_claude_and_cursor() -> Result<(), Box<dyn std::error::Error>> {
    assert!(BUILTIN_DRIVER_SPECS.len() >= 12);

    assert_eq!(
        codex_app_server_args(&["model_reasoning_effort=high".to_owned()]),
        vec![
            "app-server",
            "-c",
            "thread_unload_delay_secs=0",
            "-c",
            "model_reasoning_effort=high",
        ]
    );
    let tg = codex_text_gen_args("/tmp/schema.json", "/tmp/out.txt", Some("gpt-5.4"), None);
    assert!(tg.contains(&"--ephemeral".to_owned()));
    assert!(tg.contains(&"read-only".to_owned()));
    let mut codec = AcpStdioCodec::new();
    let (_, thread_line) =
        codex_thread_start_envelope(&mut codec, "/repo", Some("gpt-5.4"), "never", "read-only")?;
    let (_, turn_line) = codex_turn_start_envelope(&mut codec, "thr-1", "Summarize")?;
    assert!(thread_line.contains("thread/start"));
    assert!(turn_line.contains("turn/start"));

    let dir = tempdir()?;
    let exe_path = dir.path().join("claude.exe");
    std::fs::write(&exe_path, b"mz")?;
    let shim_path = dir.path().join("claude.cmd");
    let resolved = resolve_claude_executable(&shim_path.to_string_lossy(), true, |candidate| {
        std::path::Path::new(candidate).exists()
    })?;
    assert_eq!(resolved, exe_path.to_string_lossy());
    assert!(resolve_claude_executable(r"C:\missing\claude.cmd", true, |_| false).is_err());

    let claude_env = SanitizedEnv::from_entries(
        [
            ("PATH", "C:\\bin"),
            ("CLAUDECODE", "1"),
            ("ELECTRON_RUN_AS_NODE", "1"),
        ],
        true,
    );
    let child_claude = build_claude_env(&claude_env, Some("C:\\cfg"));
    assert!(!child_claude.contains_key("CLAUDECODE"));
    assert!(!child_claude.contains_key("ELECTRON_RUN_AS_NODE"));
    assert_eq!(
        child_claude
            .get("CLAUDE_CODE_ENTRYPOINT")
            .map(String::as_str),
        Some("legioncode")
    );

    let cursor =
        cursor_isolated_sidecar_spec(&claude_env, "cursor-agent", "/tmp/cursor-db", true, true);
    assert_eq!(cursor.setting_sources.len(), 0);
    assert!(cursor.use_http1_for_agent);
    Ok(())
}

#[test]
fn builtin_drivers_cover_kiro_fx_opencode_pi_muse_and_registry()
-> Result<(), Box<dyn std::error::Error>> {
    let base_env = SanitizedEnv::from_entries([("PATH", "/usr/bin")], false);
    let kiro = kiro_launch_spec(
        &base_env,
        "kiro-cli",
        Some("/repo"),
        RuntimeMode::FullAccess,
        false,
        Some("high"),
    );
    assert_eq!(kiro.autopilot, "on");
    assert_eq!(
        kiro.launch.args,
        vec!["acp", "--agent-engine=v3", "--auth-method=cli"]
    );
    let fx = fx_launch_spec(&base_env, "fx", Some("/repo"), RuntimeMode::AutoAcceptEdits);
    assert_eq!(fx.launch.cancel_behavior, FX_ACP_CANCEL_BEHAVIOR);
    assert_eq!(fx.session_mode, "acceptEdits");
    assert!(fx_tool_call_was_cancelled(
        &ToolCallStatus::Failed,
        Some("Turn cancelled")
    ));

    let oc1_env = SanitizedEnv::from_entries([("OPENCODE_SERVER_PASSWORD", "old")], false);
    let oc1 = opencode_server_env(&oc1_env, 1, "secret-v1");
    assert_eq!(
        oc1.get("OPENCODE_SERVER_PASSWORD").map(String::as_str),
        Some("secret-v1")
    );
    let oc2 = opencode_server_env(&oc1_env, 2, "newpw");
    assert!(!oc2.contains_key("OPENCODE_SERVER_PASSWORD"));
    assert_eq!(
        oc2.get("OPENCODE_PASSWORD").map(String::as_str),
        Some("newpw")
    );
    assert_eq!(opencode_basic_auth_header("pw"), "Basic b3BlbmNvZGU6cHc=");
    let mut page = json!({
        "items": [
            { "type": "compaction", "metadata": { "recent": "summary" } },
            { "info": { "type": "compaction" } },
            { "type": "assistant", "text": "ok" }
        ]
    });
    assert_eq!(fill_compaction_recents(&mut page), 2);
    assert_eq!(page["items"][0]["recent"], "summary");
    assert_eq!(page["items"][1]["info"]["recent"], "");

    let pi_args = pi_launch_args(true, Some("sonnet"), Some("anthropic"), &[])?;
    assert_eq!(
        pi_args,
        vec![
            "--mode",
            "rpc",
            "--no-session",
            "--provider",
            "anthropic",
            "--model",
            "sonnet"
        ]
    );
    assert!(pi_launch_args(false, None, Some("anthropic"), &[]).is_err());
    assert!(pi_launch_args(false, None, None, &["--mode=text".to_owned()]).is_err());
    assert!(
        pi_probe_spec(&oc1_env, "pi")
            .args
            .contains(&"--offline".to_owned())
    );

    assert_eq!(muse_verbatim_path(r"\\?\C:\repo", true), r"C:\repo");
    assert_eq!(
        muse_verbatim_path(r"\\?\UNC\srv\share\r", true),
        r"\\srv\share\r"
    );
    let muse = muse_launch_spec(
        &SanitizedEnv::from_entries([("META_API_KEY", "secret")], true),
        "muse",
        Some(r"\\?\D:\work"),
    );
    assert!(!muse.env.contains_key("META_API_KEY"));
    assert_eq!(muse.cwd.as_deref(), Some(r"D:\work"));

    assert!(validate_registry_executable(r"C:\bin\agent.CMD", true).is_err());
    assert!(validate_registry_executable(r"C:\bin\agent.exe", true).is_ok());
    assert!(normalize_registry_command_path("../escape/bin").is_err());
    Ok(())
}

#[test]
fn grok_maintenance_handles_native_fallback_and_homebrew_detection() {
    let homebrew_input = MaintenanceResolutionInput {
        resolved_binary_path: Some("/opt/homebrew/Cellar/grok/0.5.0/bin/grok".to_owned()),
        ..Default::default()
    };
    let hb_plan = resolve_grok_maintenance(&homebrew_input, Some("grok"));
    assert!(hb_plan.command.executable.contains("brew") || hb_plan.command.executable == "grok");

    let isolated = resolve_grok_maintenance(&MaintenanceResolutionInput::default(), None);
    assert!(isolated.used_native_fallback);
    assert_eq!(isolated.command.executable, "grok");
    assert_eq!(isolated.command.args, vec!["update"]);
}
