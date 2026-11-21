//! Focused tests for the compatibility floor, manifest minimum versions,
//! Antigravity `< 1.5.0` refusal, and Codex local binary probe caching.

use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};

use t3_contracts::brand::LEGION_CODE;
use t3_protocol::provider_env::SanitizedEnv;
use t3_providers::core::{
    CompatibilityStatus, Semver, broken_version_detail, default_compatibility_policies,
    minimum_supported_version_from_manifest, parse_compatibility_policies,
    resolve_provider_compatibility,
};
use t3_providers::drivers::{
    CodexHistoryRecoveryDecision, CodexLocalProbeCache, classify_codex_managed_error,
    codex_should_recover_rejected_history, codex_stdio_mcp_server_names, codex_thread_env_name,
    codex_thread_runtime_config_overrides, dedupe_web_search_queries,
    inspect_codex_binary_fingerprint, parse_codex_cli_version, resolve_codex_launch_args,
    tokenize_cli_args,
};

#[test]
fn compatibility_policies_refuse_antigravity_below_1_5_0_and_rate_patterns() {
    let policies = default_compatibility_policies();
    for version in [
        "1.1.1",
        "1.3.0",
        "1.4.1",
        "1.4.5",
        "1.4.9",
        "agy_acp_server_1.4.5",
        "1.4.5-beta",
    ] {
        let advisory =
            resolve_provider_compatibility(&policies, None, "antigravity", Some(version), "0.1.11")
                .expect("antigravity policy present");
        assert_eq!(advisory.status, CompatibilityStatus::Broken, "{version}");
        let msg = broken_version_detail(
            "Antigravity",
            version,
            Some(&advisory),
            Some("Update it in Antigravity setup"),
        )
        .expect("broken runtime produces unified message");
        assert!(msg.contains("does not work with this T3 Code release"));
        assert!(msg.contains(">=1.5.12 <2"));
    }

    let ok =
        resolve_provider_compatibility(&policies, None, "antigravity", Some("1.5.11"), "0.1.11")
            .expect("supported policy");
    assert_eq!(ok.status, CompatibilityStatus::Supported);
    assert!(
        broken_version_detail("Antigravity", "1.5.11", Some(&ok), None).is_none(),
        "supported versions must not fail"
    );

    // Older hosts (<0.1.10) still keep their legacy compatibility policy.
    let legacy =
        resolve_provider_compatibility(&policies, None, "antigravity", Some("1.4.5"), "0.1.9")
            .expect("legacy policy");
    assert_eq!(legacy.status, CompatibilityStatus::Supported);

    // Date-tagged Cursor and revision-tagged Muse resolve via manifest versionPattern,
    // while bare prereleases without a pattern remain Unknown.
    let cursor = resolve_provider_compatibility(
        &policies,
        None,
        "cursor",
        Some("2026.05.09-abc123f"),
        "0.1.11",
    )
    .expect("cursor policy");
    assert_eq!(cursor.status, CompatibilityStatus::Supported);

    let muse =
        resolve_provider_compatibility(&policies, None, "muse", Some("1.4.2-R12.1"), "0.1.11")
            .expect("muse policy");
    assert_eq!(muse.status, CompatibilityStatus::Supported);

    let prerelease =
        resolve_provider_compatibility(&policies, None, "codex", Some("0.156.0-beta.1"), "0.1.11")
            .expect("codex policy");
    assert_eq!(prerelease.status, CompatibilityStatus::Unknown);
}

#[test]
fn opencode_and_pi_minimum_versions_read_from_manifest() {
    let policies = default_compatibility_policies();
    assert_eq!(
        minimum_supported_version_from_manifest(&policies, None, "opencode", "0.1.11"),
        Some(Semver {
            major: 1,
            minor: 14,
            patch: 19
        })
    );
    assert_eq!(
        minimum_supported_version_from_manifest(&policies, None, "pi", "0.1.11"),
        Some(Semver {
            major: 0,
            minor: 84,
            patch: 3
        })
    );

    // OpenCode prerelease/build tags resolve base version via versionPattern.
    for (ver, expected) in [
        ("1.13.0-beta.1", CompatibilityStatus::Broken),
        ("1.13.0+abc123", CompatibilityStatus::Broken),
        ("2.1.0-rc.1", CompatibilityStatus::Supported),
    ] {
        let adv = resolve_provider_compatibility(&policies, None, "opencode", Some(ver), "0.1.11")
            .expect("opencode policy");
        assert_eq!(adv.status, expected, "{ver}");
    }

    // Dynamic manifest override replaces the bundled policy while leaving unrelated drivers intact.
    let custom = parse_compatibility_policies(
        r#"{"compatibility":[
            {"driver":"pi","t3CodeRange":">=0.0.46","recommendedRange":">=1.2.0","ranges":[{"range":">=1.2.0","status":"supported"},{"range":"<1.2.0","status":"broken"}]}
        ]}"#,
    )
    .expect("valid manifest json");
    assert_eq!(
        minimum_supported_version_from_manifest(&custom, Some(&policies), "pi", "0.1.11"),
        Some(Semver {
            major: 1,
            minor: 2,
            patch: 0
        })
    );
    let broken_pi =
        resolve_provider_compatibility(&custom, Some(&policies), "pi", Some("0.9.0"), "0.1.11")
            .expect("custom pi");
    assert_eq!(broken_pi.status, CompatibilityStatus::Broken);
}

#[test]
fn codex_local_probe_cache_persists_native_and_invalidates_on_change() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bin_dir = dir.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("mkdir bin");
    let exe_name = if cfg!(windows) { "codex.exe" } else { "codex" };
    let host_name = if cfg!(windows) {
        "codex-code-mode-host.exe"
    } else {
        "codex-code-mode-host"
    };
    let exe = bin_dir.join(exe_name);
    let host = bin_dir.join(host_name);
    fs::write(&exe, b"binary-v1").expect("write exe");
    fs::write(&host, b"host").expect("write host");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&host, fs::Permissions::from_mode(0o755)).expect("chmod");
    }

    let fp = inspect_codex_binary_fingerprint(&exe, cfg!(windows)).expect("fingerprint");
    assert!(fp.should_persist());

    let persist_path = dir.path().join("caches/codex-local-probe.json");
    let cache = CodexLocalProbeCache::with_persist_path(&persist_path);
    let calls = AtomicUsize::new(0);

    let first = cache
        .resolve_version_with(&exe, cfg!(windows), |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok::<_, ()>(Some("codex-cli 0.156.1\n".to_owned()))
        })
        .expect("first resolve");
    assert_eq!(first.as_deref(), Some("0.156.1"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // Second instance with empty in-memory map loads persisted probe instead of re-probing.
    let restarted = CodexLocalProbeCache::with_persist_path(&persist_path);
    let second = restarted
        .resolve_version_with(&exe, cfg!(windows), |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok::<_, ()>(Some("codex-cli 0.999.0\n".to_owned()))
        })
        .expect("persisted hit");
    assert_eq!(second.as_deref(), Some("0.156.1"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // Changing binary size invalidates cache and triggers a fresh probe.
    fs::write(&exe, b"binary-v2-longer").expect("update exe");
    let third = restarted
        .resolve_version_with(&exe, cfg!(windows), |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok::<_, ()>(Some("codex-cli 0.157.0\n".to_owned()))
        })
        .expect("invalidated hit");
    assert_eq!(third.as_deref(), Some("0.157.0"));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        parse_codex_cli_version("codex-cli 0.158.0\r\n").as_deref(),
        Some("0.158.0")
    );
}

#[test]
fn codex_mcp_env_launch_args_web_search_and_history_recovery() {
    let mcp = serde_json::json!({
        "local": { "command": "npx", "args": ["srv"] },
        "remote": { "url": "https://example.com/mcp" },
        "bad.dotted": { "command": "foo" },
    });
    let names = codex_stdio_mcp_server_names(Some(&mcp));
    assert_eq!(names, vec!["local".to_owned()]);

    let overrides = codex_thread_runtime_config_overrides(Some("thr-42"), &names, None);
    assert_eq!(
        overrides
            .get("shell_environment_policy.set.T3CODE_THREAD_ID")
            .and_then(|v| v.as_str()),
        Some("thr-42")
    );
    assert_eq!(
        overrides
            .get("mcp_servers.local.env.T3CODE_THREAD_ID")
            .and_then(|v| v.as_str()),
        Some("thr-42")
    );
    assert_eq!(codex_thread_env_name(Some(LEGION_CODE)), "LEGION_THREAD_ID");

    let host_override = std::collections::BTreeMap::from([(
        "T3CODE_CODEX_LAUNCH_ARGS".to_owned(),
        "--cloud-skills-snapshot skills.json\\ ".to_owned(),
    )]);
    let raw = t3_providers::drivers::resolve_codex_launch_args_with_lookup(
        Some("--strict-config"),
        |key| host_override.get(key).cloned(),
    );
    assert_eq!(
        tokenize_cli_args(&raw),
        vec!["--cloud-skills-snapshot", "skills.json "]
    );

    let sanitized =
        SanitizedEnv::from_entries([("T3_CODEX_LAUNCH_ARGS", "--flag value\\ ")], cfg!(windows));
    assert_eq!(
        tokenize_cli_args(&resolve_codex_launch_args(
            Some("--strict-config"),
            &sanitized
        )),
        vec!["--flag", "value "]
    );

    let blank_env = SanitizedEnv::from_entries([("T3_CODEX_LAUNCH_ARGS", " \t ")], false);
    assert_eq!(
        resolve_codex_launch_args(Some(" --strict-config "), &blank_env),
        "--strict-config"
    );

    assert_eq!(
        dedupe_web_search_queries([Some("fifa"), Some(" fifa "), Some("  "), Some("tickets")]),
        vec!["fifa".to_owned(), "tickets".to_owned()]
    );

    let raw_err = r#"{"error":{"code":"invalid_encrypted_content","message":"bad"},"headers":{"x-codex-turn-state":"secret"}}"#;
    let managed = classify_codex_managed_error(raw_err).expect("classified");
    assert_eq!(managed.code, "invalid_encrypted_content");
    assert!(!managed.message.contains("secret"));
    assert_eq!(
        codex_should_recover_rejected_history(Some("other"), raw_err, false, false),
        CodexHistoryRecoveryDecision::RecoverInFreshSession
    );
    assert_eq!(
        codex_should_recover_rejected_history(
            Some("invalid_encrypted_content"),
            "err",
            false,
            true
        ),
        CodexHistoryRecoveryDecision::BlockedByPendingBackgroundWork
    );
    assert_eq!(
        codex_should_recover_rejected_history(
            Some("invalid_encrypted_content"),
            "err",
            true,
            false
        ),
        CodexHistoryRecoveryDecision::ContinueNormalResume
    );
}
