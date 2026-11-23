//! Integration tests for `--version`, `status --json`, and `policy` CLI commands.

mod support;

use std::fs;
use std::process::Command;

use legion_contracts::policy::{ManagedPolicyDocument, policy_fingerprint_12hex};
use legion_persistence::ownership::{
    ManagedPolicyRecord, ManagedPolicyState, PersistedServerRuntimeState,
    persist_server_runtime_state,
};
use legion_persistence::paths::ServerDerivedPaths;
use serde_json::Value;

use support::{TestResult, isolated_env, run_legioncode};

#[test]
fn version_flags_print_exact_version_and_match_node() -> TestResult {
    let temp = tempfile::tempdir()?;
    let env = isolated_env(temp.path(), &[]);

    let expected_version = format!("legioncode v{}\n", env!("CARGO_PKG_VERSION"));
    for flag in ["--version", "-v"] {
        let out = run_legioncode(&[flag], &env)?;
        assert!(out.status.success());
        assert_eq!(String::from_utf8(out.stdout)?, expected_version);
    }

    if let Ok(node_out) = Command::new("node")
        .args([legion_cli::fallback::LEGACY_BIN_TS, "--version"])
        .output()
        && node_out.status.success()
    {
        let rust_out = run_legioncode(&["--version"], &env)?;
        let rust_s = String::from_utf8(rust_out.stdout)?;
        let node_s = String::from_utf8(node_out.stdout)?;
        assert!(rust_s.starts_with("legioncode v") && rust_s.ends_with('\n'));
        assert!(node_s.starts_with("legioncode v") && node_s.ends_with('\n'));
    }
    Ok(())
}

#[test]
fn status_json_parses_service_env_and_supports_legioncode_home() -> TestResult {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let base_dir = temp.path().join("legionhome");
    fs::create_dir_all(&home)?;
    fs::create_dir_all(&base_dir)?;

    let env = isolated_env(&home, &[]);
    let out_empty = run_legioncode(
        &[
            "status",
            "--base-dir",
            &base_dir.to_string_lossy(),
            "--json",
        ],
        &env,
    )?;
    assert!(out_empty.status.success());
    let stdout_empty = String::from_utf8(out_empty.stdout)?;
    assert!(stdout_empty.ends_with('\n'));
    let parsed_empty: Value = serde_json::from_str(&stdout_empty)?;
    assert_eq!(
        parsed_empty,
        serde_json::json!({
            "service": {
                "supported": true,
                "installed": false,
                "installedVersion": null,
                "otherBaseDir": null
            },
            "connect": null,
            "lan": {
                "enabled": false,
                "inheritedDefault": false,
                "hostOverride": null
            }
        })
    );
    // Does not mutate empty base_dir by creating userdata/sqlite.
    assert!(!base_dir.join("userdata").exists());

    fs::write(
        base_dir.join("service.env"),
        "# comment\nLEGIONCODE_LAN=\"yes\"\nLEGIONCODE_HOST='10.20.30.40' # tailnet\n",
    )?;
    let base_str = base_dir.to_string_lossy().into_owned();
    let env_home = isolated_env(&home, &[("LEGIONCODE_HOME", &base_str)]);
    let out_home = run_legioncode(&["status", "--json"], &env_home)?;
    assert!(out_home.status.success());
    let parsed_home: Value = serde_json::from_str(&String::from_utf8(out_home.stdout)?)?;
    assert_eq!(parsed_home["lan"]["enabled"], true);
    assert_eq!(parsed_home["lan"]["inheritedDefault"], false);
    assert_eq!(parsed_home["lan"]["hostOverride"], "10.20.30.40");

    fs::write(
        base_dir.join("service.env"),
        "LEGIONCODE_LAN=off\nLEGIONCODE_HOST=\"\"\n",
    )?;
    let out_off = run_legioncode(&["status", "--base-dir", &base_str, "--json"], &env)?;
    let parsed_off: Value = serde_json::from_str(&String::from_utf8(out_off.stdout)?)?;
    assert_eq!(parsed_off["lan"]["enabled"], false);
    assert_eq!(parsed_off["lan"]["hostOverride"], serde_json::Value::Null);

    // Simulated installed boot service serving this home (without LEGIONCODE_LAN -> inheritedDefault: true).
    fs::remove_file(base_dir.join("service.env"))?;
    fs::create_dir_all(base_dir.join("runtime"))?;
    fs::write(
        base_dir.join("runtime").join("service-state.json"),
        r#"{"protocol":4,"activeVersion":"0.1.10"}"#,
    )?;
    write_test_boot_unit(&home, &base_str)?;
    let out_svc = run_legioncode(&["status", "--base-dir", &base_str, "--json"], &env)?;
    assert!(out_svc.status.success());
    let parsed_svc: Value = serde_json::from_str(&String::from_utf8(out_svc.stdout)?)?;
    assert_eq!(parsed_svc["service"]["installed"], true);
    assert_eq!(parsed_svc["service"]["installedVersion"], "0.1.10");
    assert_eq!(parsed_svc["service"]["otherBaseDir"], Value::Null);
    assert_eq!(parsed_svc["lan"]["enabled"], true);
    assert_eq!(parsed_svc["lan"]["inheritedDefault"], true);

    // Different base_dir -> otherBaseDir populated and serves_this_home = false.
    let other_dir = temp.path().join("other-home");
    let out_other = run_legioncode(
        &[
            "status",
            "--base-dir",
            &other_dir.to_string_lossy(),
            "--json",
        ],
        &env,
    )?;
    assert!(out_other.status.success());
    let parsed_other: Value = serde_json::from_str(&String::from_utf8(out_other.stdout)?)?;
    assert_eq!(parsed_other["service"]["installed"], true);
    assert_eq!(parsed_other["service"]["otherBaseDir"], base_str);
    assert_eq!(parsed_other["lan"]["enabled"], false);
    assert_eq!(parsed_other["lan"]["inheritedDefault"], false);
    Ok(())
}

fn write_test_boot_unit(home: &std::path::Path, base_dir: &str) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        let dir = home.join("legioncode");
        fs::create_dir_all(&dir)?;
        let xml = format!(
            "<Task><Actions><Exec><Arguments>--headless \"legioncode.exe\" \"__service-launcher\" \"--base-dir\" \"{base_dir}\"</Arguments></Exec></Actions></Task>"
        );
        fs::write(dir.join("legioncode-task.xml"), xml)
    }
    #[cfg(target_os = "macos")]
    {
        let dir = home.join("Library").join("LaunchAgents");
        fs::create_dir_all(&dir)?;
        let plist = format!(
            "<plist><dict><key>LEGIONCODE_HOME</key><string>{base_dir}</string></dict></plist>"
        );
        fs::write(dir.join("dev.legioncode.service.plist"), plist)
    }
    #[cfg(target_os = "linux")]
    {
        let dir = home.join("systemd").join("user");
        fs::create_dir_all(&dir)?;
        let unit = format!("[Service]\nEnvironment=LEGIONCODE_HOME={base_dir}\n");
        fs::write(dir.join("legioncode.service"), unit)
    }
}

#[test]
fn policy_check_matches_node_trailing_double_newline_and_rejects_bad_files() -> TestResult {
    let temp = tempfile::tempdir()?;
    let env = isolated_env(temp.path(), &[]);
    let policy_path = temp.path().join("policy.json");
    fs::write(
        &policy_path,
        r#"{
          "schemaVersion": 1,
          "organization": "Acme Corp",
          "supportUrl": "https://acme.example/help",
          "settings": {
            "telemetry.enabled": { "value": false, "locked": true }
          },
          "limits": {
            "maxRuntimeMode": "auto-accept-edits"
          }
        }"#,
    )?;

    let policy_arg = policy_path.to_string_lossy().into_owned();
    let out = run_legioncode(&["policy", "check", &policy_arg], &env)?;
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout)?;
    assert!(stdout.ends_with("\n\n"));
    assert!(stdout.contains(&format!("Managed by Acme Corp ({policy_arg}, fingerprint ")));
    assert!(stdout.contains("Support: https://acme.example/help"));
    assert!(stdout.contains("  telemetry.enabled = false (locked)"));

    // Verify byte-for-byte parity with Node CLI on the same file when node deps are installed.
    if let Ok(node_out) = Command::new("node")
        .args([
            legion_cli::fallback::LEGACY_BIN_TS,
            "policy",
            "check",
            &policy_arg,
        ])
        .output()
        && node_out.status.success()
    {
        assert_eq!(stdout, String::from_utf8(node_out.stdout)?);
    }

    let missing_arg = temp
        .path()
        .join("missing.json")
        .to_string_lossy()
        .into_owned();
    let out_missing = run_legioncode(&["policy", "check", &missing_arg], &env)?;
    assert_eq!(out_missing.status.code(), Some(1));
    assert!(
        String::from_utf8(out_missing.stderr)?
            .contains(&format!("No policy file at {missing_arg}."))
    );

    let invalid_path = temp.path().join("invalid.json");
    fs::write(
        &invalid_path,
        r#"{"schemaVersion":1,"organization":"Acme","unknownKey":1}"#,
    )?;
    let out_invalid = run_legioncode(&["policy", "check", &invalid_path.to_string_lossy()], &env)?;
    assert_eq!(out_invalid.status.code(), Some(1));
    assert!(
        String::from_utf8(out_invalid.stderr)?.contains("LegionCode cannot use the managed policy")
    );
    Ok(())
}

#[test]
fn policy_show_and_templates_handle_running_server_and_custom_policies() -> TestResult {
    let temp = tempfile::tempdir()?;
    let base_dir = temp.path().join("legionhome");
    let policy_file = temp.path().join("managed.json");
    let policy_str = policy_file.to_string_lossy().into_owned();
    let base_str = base_dir.to_string_lossy().into_owned();

    // Missing policy file -> unmanaged message + no running server.
    let env_missing = isolated_env(
        temp.path(),
        &[("LEGIONCODE_MANAGED_POLICY_FILE", &policy_str)],
    );
    let out_none = run_legioncode(&["policy", "show", "--base-dir", &base_str], &env_missing)?;
    assert!(out_none.status.success());
    let text_none = String::from_utf8(out_none.stdout)?;
    assert!(
        text_none
            .contains("No managed policy on this machine. Every setting belongs to the user.\n\n")
    );
    assert!(text_none.contains("No LegionCode server is running from this LegionCode home."));

    // Installed policy + live server matching fingerprint.
    let doc_raw = r#"{"schemaVersion":1,"organization":"Policy Org","limits":{"telemetry":false}}"#;
    fs::write(&policy_file, doc_raw)?;
    let doc: ManagedPolicyDocument = serde_json::from_str(doc_raw)?;
    let fp = policy_fingerprint_12hex(&doc)?;

    let paths = ServerDerivedPaths::derive(&base_dir, false, true);
    let mut state = PersistedServerRuntimeState::for_current_process(3773, None);
    state.managed_policy = ManagedPolicyState::Loaded(ManagedPolicyRecord {
        source: policy_str.clone(),
        organization: "Policy Org".to_owned(),
        fingerprint: fp,
    });
    persist_server_runtime_state(&paths.server_runtime_state_path, &state)?;

    let out_loaded = run_legioncode(&["policy", "show", "--base-dir", &base_str], &env_missing)?;
    assert!(out_loaded.status.success());
    let text_loaded = String::from_utf8(out_loaded.stdout)?;
    assert!(text_loaded.contains("Managed by Policy Org"));
    assert!(text_loaded.contains("loaded this policy."));

    // Policy templates (sample + custom + null error).
    let tpl_default = temp.path().join("tpl-default");
    let out_tpl = run_legioncode(
        &["policy", "templates", &tpl_default.to_string_lossy()],
        &env_missing,
    )?;
    assert!(out_tpl.status.success());
    assert!(
        tpl_default
            .join("windows")
            .join("legioncode.admx")
            .is_file()
    );
    assert!(
        tpl_default
            .join("windows")
            .join("en-US")
            .join("legioncode.adml")
            .is_file()
    );
    let mobileconfig = tpl_default.join("macos").join("legioncode.mobileconfig");
    assert!(mobileconfig.is_file());
    assert!(fs::read_to_string(&mobileconfig)?.contains("Example Corp"));

    let tpl_custom = temp.path().join("tpl-custom");
    let out_custom = run_legioncode(
        &[
            "policy",
            "templates",
            &tpl_custom.to_string_lossy(),
            "--policy",
            &policy_str,
        ],
        &env_missing,
    )?;
    assert!(out_custom.status.success());
    let custom_plist =
        fs::read_to_string(tpl_custom.join("macos").join("legioncode.mobileconfig"))?;
    assert!(custom_plist.contains("Policy Org"));

    let null_policy = temp.path().join("null.json");
    fs::write(
        &null_policy,
        r#"{"schemaVersion":1,"organization":"Null Org","settings":{"ui.theme":{"value":null,"locked":false}}}"#,
    )?;
    let out_null = run_legioncode(
        &[
            "policy",
            "templates",
            &temp.path().join("tpl-null").to_string_lossy(),
            "--policy",
            &null_policy.to_string_lossy(),
        ],
        &env_missing,
    )?;
    assert_eq!(out_null.status.code(), Some(1));
    assert!(String::from_utf8(out_null.stderr)?.contains("cannot hold"));
    Ok(())
}
