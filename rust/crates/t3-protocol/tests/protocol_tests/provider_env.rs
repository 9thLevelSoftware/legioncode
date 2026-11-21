use std::collections::BTreeSet;

use proptest::prelude::*;
use serde_json::json;
use t3_protocol::provider_env::{
    MAX_NDJSON_LINE_BYTES, MaintenanceInstaller, MaintenanceProbe, MaintenanceResolutionInput,
    MaintenanceShimMode, MaintenanceTier, NdjsonCodecError, NdjsonLineCodec,
    ProviderMaintenanceDefinition, SanitizedEnv, TerminalResumeStatus, TerminalSessionKey,
    TerminalSessionKeys, make_targeted_update_action, resolve_latest_update_action,
    resolve_provider_maintenance,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn sanitized_env_blocks_ambient_leaks_and_handles_windows_case_bug_12() {
    let linux = SanitizedEnv::from_entries(
        [
            ("PATH", "/tmp/AppDir/usr/bin:/usr/local/bin:/usr/bin"),
            ("LD_LIBRARY_PATH", "/tmp/AppDir/usr/lib:/opt/lib"),
            ("APPIMAGE", "/tmp/T3Code.AppImage"),
            ("APPDIR", "/tmp/AppDir"),
            ("ARGV0", "t3code"),
            ("OWD", "/home/user"),
            ("T3CODE_HOME", "/secret/t3"),
            ("T3CODE_DEV_AUTH_TOKEN", "dev-secret"),
            ("VITE_WS_URL", "ws://localhost:9999"),
            ("PORT", "3773"),
            ("ELECTRON_RUN_AS_NODE", "1"),
            ("TMUX_PANE", "%1"),
            ("ZELLIJ", "0"),
            ("HOME", "/home/user"),
        ],
        false,
    );

    for leaked in [
        "APPIMAGE",
        "APPDIR",
        "ARGV0",
        "OWD",
        "T3CODE_HOME",
        "T3CODE_DEV_AUTH_TOKEN",
        "VITE_WS_URL",
        "PORT",
        "ELECTRON_RUN_AS_NODE",
        "TMUX_PANE",
        "ZELLIJ",
    ] {
        assert_eq!(linux.get(leaked), None, "{leaked} must be stripped");
    }
    assert_eq!(linux.get("PATH"), Some("/usr/local/bin:/usr/bin"));
    assert_eq!(linux.get("LD_LIBRARY_PATH"), Some("/opt/lib"));
    assert_eq!(linux.get("HOME"), Some("/home/user"));

    let term = linux.for_terminal([("T3CODE_SECRET", "nope"), ("TERM", "xterm-256color")]);
    assert_eq!(term.get("T3CODE_SECRET"), None);
    assert_eq!(term.get("TERM"), Some("xterm-256color"));

    // Windows uppercase key precedence & managed policy override immunity.
    let win = SanitizedEnv::from_entries(
        [("Path", r"C:\Windows"), ("PATH", r"C:\Windows;C:\Tools")],
        true,
    );
    assert_eq!(win.as_map().len(), 1);
    assert_eq!(win.get("PATH"), Some(r"C:\Windows;C:\Tools"));

    let provider = win.for_provider_instance(
        [
            ("OPENAI_API_KEY", "user-key"),
            ("T3CODE_HOME", "leak"),
            ("path", r"C:\UserOnly"),
        ],
        [("OPENAI_API_KEY", "managed-locked"), ("VITE_BAD", "x")],
    );
    assert_eq!(provider.get("OPENAI_API_KEY"), Some("managed-locked"));
    assert_eq!(provider.get("T3CODE_HOME"), None);
    assert_eq!(provider.get("VITE_BAD"), None);
    assert_eq!(provider.get("PATH"), Some(r"C:\UserOnly"));
}

#[test]
fn ndjson_codec_uses_strict_lf_and_enforces_128mib_cap() -> TestResult {
    assert_eq!(MAX_NDJSON_LINE_BYTES, 128 * 1024 * 1024);

    let mut codec = NdjsonLineCodec::new(64);
    let payload = "{\"msg\":\"keep\rinside\u{2028}and\u{2029}here\"}\r\n  \n";
    let bytes = payload.as_bytes();
    // Split mid-UTF-8 sequence intentionally to verify byte buffering before decoding.
    assert_eq!(codec.push_bytes(&bytes[..15])?, Vec::<String>::new());
    let lines = codec.push_bytes(&bytes[15..])?;
    assert_eq!(
        lines,
        vec!["{\"msg\":\"keep\rinside\u{2028}and\u{2029}here\"}"]
    );

    let oversized = vec![b'a'; 65];
    assert!(matches!(
        codec.push_bytes(&oversized),
        Err(NdjsonCodecError::LineTooLarge {
            max_bytes: 64,
            observed_bytes: 65
        })
    ));
    assert_eq!(codec.pending_bytes(), 0);
    assert_eq!(codec.encode_line(&json!({"ok": true}))?, "{\"ok\":true}\n");
    Ok(())
}

proptest! {
    #[test]
    fn ndjson_roundtrips_arbitrary_strings_containing_unicode_separators(text in "[\\x00-\\u{202f}]{0,64}") {
        let mut codec = NdjsonLineCodec::default();
        let value = json!({ "payload": text });
        let encoded = codec.encode_line(&value).map_err(|err| TestCaseError::fail(err.to_string()))?;
        let decoded: Vec<serde_json::Value> = codec
            .push_json_lines(encoded.as_bytes())
            .map_err(|err| TestCaseError::fail(err.to_string()))?;
        prop_assert_eq!(decoded, vec![value]);
    }
}

#[test]
fn maintenance_precedence_fixes_grok_and_bun_exact_pin_bug_13_bug_14() {
    let grok = ProviderMaintenanceDefinition::grok();

    // BUG-13: Grok on pnpm/bun/npm/vite-plus/yarn/volta uses JS package managers first.
    let pnpm_caps = resolve_provider_maintenance(
        &grok,
        &MaintenanceResolutionInput {
            resolved_binary_path: Some("/Users/dev/.local/share/pnpm/grok".to_owned()),
            ..Default::default()
        },
    );
    assert_eq!(pnpm_caps.installer, MaintenanceInstaller::PnpmGlobal);
    assert_eq!(pnpm_caps.resolved_tier, MaintenanceTier::PnpmGlobal);
    assert!(pnpm_caps.supports_exact_pin);
    assert_eq!(
        resolve_latest_update_action(&pnpm_caps, Some("1.4.0")).map(|c| c.args),
        Some(vec![
            "add".to_owned(),
            "-g".to_owned(),
            "@xai-official/grok@1.4.0".to_owned()
        ])
    );

    // BUG-14: bun-global keeps supports_exact_pin = false and rejects targeted pins.
    let bun_caps = resolve_provider_maintenance(
        &grok,
        &MaintenanceResolutionInput {
            resolved_binary_path: Some("/home/dev/.bun/bin/grok".to_owned()),
            ..Default::default()
        },
    );
    assert_eq!(bun_caps.installer, MaintenanceInstaller::BunGlobal);
    assert_eq!(bun_caps.resolved_tier, MaintenanceTier::BunGlobal);
    assert!(!bun_caps.supports_exact_pin);
    assert_eq!(make_targeted_update_action(&bun_caps, "1.4.0"), None);
    assert_eq!(
        resolve_latest_update_action(&bun_caps, Some("1.4.0")).map(|c| c.args),
        Some(vec![
            "i".to_owned(),
            "-g".to_owned(),
            "@xai-official/grok@latest".to_owned()
        ])
    );

    // Tier 1 Vite+ system_first unwrap jumps to the unwrapped system npm-global install.
    assert_vite_plus_unwrap(&grok);
}

fn assert_vite_plus_unwrap(grok: &ProviderMaintenanceDefinition) {
    let unwrapped_npm = resolve_provider_maintenance(
        grok,
        &MaintenanceResolutionInput {
            resolved_binary_path: Some("/home/dev/.vite-plus/bin/grok".to_owned()),
            vite_plus_shim_mode: MaintenanceShimMode::SystemFirst,
            vite_plus_unwrapped_path: Some("/opt/node/bin/grok".to_owned()),
            vite_plus_unwrapped_target_path: Some(
                "/opt/node/lib/node_modules/@xai-official/grok/cli.js".to_owned(),
            ),
            ..Default::default()
        },
    );
    assert!(unwrapped_npm.shim_unwrapped);
    assert_eq!(
        unwrapped_npm.installer,
        MaintenanceInstaller::NpmGlobal {
            prefix: Some("/opt/node".to_owned())
        }
    );
}

#[test]
fn maintenance_os_tiers_and_fallbacks_preserve_exact_order() {
    let grok = ProviderMaintenanceDefinition::grok();
    // Tier 2 WinGet > Chocolatey, Tier 5 Nix, Tier 14 Homebrew, Tier 15 Native fallback.
    let claude = ProviderMaintenanceDefinition::claude_agent();
    let winget_caps = resolve_provider_maintenance(
        &claude,
        &MaintenanceResolutionInput {
            is_windows: true,
            ownership_probes: BTreeSet::from([
                MaintenanceProbe::WinGet,
                MaintenanceProbe::Chocolatey,
                MaintenanceProbe::Homebrew,
            ]),
            ..Default::default()
        },
    );
    assert_eq!(winget_caps.installer, MaintenanceInstaller::WinGet);
    assert_eq!(winget_caps.resolved_tier, MaintenanceTier::WinGet);

    let nix_caps = resolve_provider_maintenance(
        &grok,
        &MaintenanceResolutionInput {
            resolved_target_path: Some("/nix/store/abc123-grok/bin/grok".to_owned()),
            ownership_probes: BTreeSet::from([MaintenanceProbe::Homebrew]),
            ..Default::default()
        },
    );
    assert_eq!(nix_caps.installer, MaintenanceInstaller::Nix);

    let brew_caps = resolve_provider_maintenance(
        &grok,
        &MaintenanceResolutionInput {
            resolved_binary_path: Some("/opt/homebrew/bin/grok".to_owned()),
            ownership_probes: BTreeSet::from([MaintenanceProbe::Homebrew]),
            ..Default::default()
        },
    );
    assert_eq!(brew_caps.installer, MaintenanceInstaller::Homebrew);
    assert_eq!(
        brew_caps.update_command.map(|c| c.args),
        Some(vec![
            "upgrade".to_owned(),
            "--cask".to_owned(),
            "grok".to_owned()
        ])
    );

    let native_fallback = resolve_provider_maintenance(
        &grok,
        &MaintenanceResolutionInput {
            resolved_binary_path: Some("/usr/local/bin/grok".to_owned()),
            ..Default::default()
        },
    );
    assert_eq!(
        native_fallback.installer,
        MaintenanceInstaller::NativeFallback
    );
    assert_eq!(native_fallback.resolved_tier, MaintenanceTier::Fallback);
}

#[test]
fn terminal_session_keys_separates_lru_eviction_from_close_bug_08() {
    let mut keys = TerminalSessionKeys::with_capacity(1);
    let k1 = TerminalSessionKey::new("thread-1", "term-1");
    let k2 = TerminalSessionKey::new("thread-1", "term-2");

    assert_eq!(
        TerminalSessionKey::from_wire_key(&k1.to_wire_key()).as_ref(),
        Some(&k1)
    );

    keys.open(k1.clone());
    assert_eq!(
        keys.mark_inactive(k1.clone()),
        Vec::<TerminalSessionKey>::new()
    );
    keys.open(k2.clone());
    let spilled = keys.mark_inactive(k2.clone());
    assert_eq!(spilled, vec![k1.clone()]);

    // Evicted key remains resumable!
    assert_eq!(keys.status(&k1), TerminalResumeStatus::EvictedResumable);
    assert!(keys.is_evicted(&k1));
    assert!(!keys.is_closed(&k1));
    assert!(keys.can_resume(&k1));

    // Explicit close disables resume cleanly.
    keys.close(k1.clone());
    assert_eq!(keys.status(&k1), TerminalResumeStatus::Closed);
    assert!(!keys.is_evicted(&k1));
    assert!(keys.is_closed(&k1));
    assert!(!keys.can_resume(&k1));

    // Reopening clears closed state.
    keys.open(k1.clone());
    assert!(keys.can_resume(&k1));
}
