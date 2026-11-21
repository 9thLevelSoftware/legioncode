//! Offline & sandboxed tests for `t3 up`, `t3 service` same-home operations,
//! `t3 install`, `t3 uninstall`, and `t3 update`.

use std::fs;
use std::path::Path;

use serde_json::{Value, json};
use t3_persistence::service_launcher::{ServiceState, service_state_path, write_service_state};

use crate::support::{TestResult, isolated_env, run_t3};

const NONEXISTENT_BIN_TS: &str = r"Z:\nonexistent\bin.ts";

pub(crate) fn create_fake_service_tools(bin_dir: &Path) -> TestResult {
    fs::create_dir_all(bin_dir)?;
    #[cfg(windows)]
    fs::write(bin_dir.join("schtasks.cmd"), "@echo off\r\nexit /b 0\r\n")?;
    #[cfg(target_os = "linux")]
    create_unix_tool(bin_dir, "systemctl")?;
    #[cfg(target_os = "macos")]
    create_unix_tool(bin_dir, "launchctl")?;
    Ok(())
}

#[cfg(not(windows))]
fn create_unix_tool(dir: &Path, name: &str) -> TestResult {
    use std::os::unix::fs::PermissionsExt as _;
    let path = dir.join(name);
    fs::write(&path, "#!/bin/sh\nexit 0\n")?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
    Ok(())
}

#[test]
fn up_and_same_home_service_ops_work_offline_with_sandboxed_unit() -> TestResult {
    let temp = tempfile::tempdir()?;
    let fake_bin = temp.path().join("fakebin");
    create_fake_service_tools(&fake_bin)?;
    let unit_path = temp.path().join("sandbox-unit").join("t3code-unit");
    let home = temp.path().join("up-home");
    let home_str = home.to_string_lossy().into_owned();
    let unit_str = unit_path.to_string_lossy().into_owned();
    let path_str = fake_bin.to_string_lossy().into_owned();

    let env = isolated_env(
        temp.path(),
        &[
            ("PATH", &path_str),
            ("T3_BIN_TS", NONEXISTENT_BIN_TS),
            ("T3CODE_BOOT_SERVICE_UNIT_PATH", &unit_str),
        ],
    );

    // Non-TTY `t3 up` without access flag rejects.
    let no_tty = run_t3(&["up", "--base-dir", &home_str], &env)?;
    assert_eq!(no_tty.status.code(), Some(1));
    assert!(String::from_utf8(no_tty.stderr)?.contains("Choose access with --lan"));

    // `--connect` rejected cleanly when public config missing.
    let no_connect = run_t3(&["up", "--connect", "--base-dir", &home_str], &env)?;
    assert_eq!(no_connect.status.code(), Some(1));
    assert!(String::from_utf8(no_connect.stderr)?.contains("T3 Connect is unavailable"));

    // `--no-lan` when not installed updates `service.env` and skips service install.
    let off_out = run_t3(&["up", "--no-lan", "--base-dir", &home_str], &env)?;
    assert_eq!(off_out.status.code(), Some(0));
    assert!(String::from_utf8(off_out.stdout)?.contains("Remote access is off."));
    assert!(fs::read_to_string(home.join("service.env"))?.contains("T3CODE_LAN=false"));
    assert!(!unit_path.exists());

    // `--lan` writes `T3CODE_LAN=true` and installs sandboxed unit + service state.
    let lan_out = run_t3(&["up", "--lan", "--base-dir", &home_str], &env)?;
    assert_eq!(lan_out.status.code(), Some(0));
    let lan_stdout = String::from_utf8(lan_out.stdout)?;
    assert!(lan_stdout.contains("Service installed"));
    assert!(lan_stdout.contains("pairing code only"));
    assert!(fs::read_to_string(home.join("service.env"))?.contains("T3CODE_LAN=true"));
    assert!(unit_path.exists());

    // Same-home `service status`, `service update`, `service restart`, `service uninstall`.
    let st_out = run_t3(&["service", "status", "--base-dir", &home_str], &env)?;
    assert_eq!(st_out.status.code(), Some(0));
    assert!(String::from_utf8(st_out.stdout)?.contains("T3 Code service"));

    let upd_out = run_t3(&["service", "update", "--base-dir", &home_str], &env)?;
    assert_eq!(upd_out.status.code(), Some(0));
    assert!(String::from_utf8(upd_out.stdout)?.contains("`t3 service update` is deprecated"));

    let rst_out = run_t3(&["service", "restart", "--base-dir", &home_str], &env)?;
    assert_eq!(rst_out.status.code(), Some(0));
    assert!(String::from_utf8(rst_out.stdout)?.contains("Restarted the T3 Code service"));

    // Downgrade protection on `service install`.
    write_service_state(
        &service_state_path(&home),
        &ServiceState {
            protocol: 4,
            active_version: "99.0.0".to_owned(),
            update: None,
        },
    )?;
    let blocked = run_t3(&["service", "install", "--base-dir", &home_str], &env)?;
    assert_eq!(blocked.status.code(), Some(1));
    assert!(String::from_utf8(blocked.stderr)?.contains("--allow-downgrade"));

    let forced = run_t3(
        &[
            "service",
            "install",
            "--allow-downgrade",
            "--base-dir",
            &home_str,
        ],
        &env,
    )?;
    assert_eq!(forced.status.code(), Some(0));

    let rm_out = run_t3(&["service", "uninstall", "--base-dir", &home_str], &env)?;
    assert_eq!(rm_out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(rm_out.stdout)?,
        "Removed the T3 Code service.\n"
    );
    assert!(!unit_path.exists());
    Ok(())
}

#[test]
fn install_and_uninstall_standalone_shims_offline() -> TestResult {
    let temp = tempfile::tempdir()?;
    let empty_bin = temp.path().join("empty-bin");
    fs::create_dir_all(&empty_bin)?;
    let empty_str = empty_bin.to_string_lossy().into_owned();

    // 1. Non-standalone binary cannot self-install.
    let bin_dir = temp.path().join("install-bin");
    let bin_str = bin_dir.to_string_lossy().into_owned();
    let non_standalone_env = isolated_env(
        temp.path(),
        &[
            ("PATH", &empty_str),
            ("T3_BIN_TS", NONEXISTENT_BIN_TS),
            ("T3CODE_INSTALL_BIN_DIR", &bin_str),
            ("T3CODE_CLI_PATH", env!("CARGO_BIN_EXE_t3")),
        ],
    );
    let err_out = run_t3(&["install"], &non_standalone_env)?;
    assert_eq!(err_out.status.code(), Some(1));
    assert!(String::from_utf8(err_out.stderr)?.contains("Only the standalone t3 can install"));

    // 2. Standalone runtime installs shim, records launcher, and refuses foreign binary.
    let home = temp.path().join("standalone-home");
    let exe_name = if cfg!(windows) { "t3.exe" } else { "t3" };
    let version = env!("CARGO_PKG_VERSION");
    let exe_path = home
        .join("runtime")
        .join("versions")
        .join(version)
        .join(exe_name);
    fs::create_dir_all(exe_path.parent().ok_or("missing version dir")?)?;
    fs::write(&exe_path, b"placeholder-exe")?;
    let exe_str = exe_path.to_string_lossy().into_owned();
    let home_str = home.to_string_lossy().into_owned();

    let standalone_env = isolated_env(
        temp.path(),
        &[
            ("PATH", &empty_str),
            ("T3_BIN_TS", NONEXISTENT_BIN_TS),
            ("T3CODE_INSTALL_BIN_DIR", &bin_str),
            ("T3CODE_CLI_PATH", &exe_str),
        ],
    );
    let inst_out = run_t3(&["install"], &standalone_env)?;
    assert_eq!(
        inst_out.status.code(),
        Some(0),
        "install stderr: {}",
        String::from_utf8_lossy(&inst_out.stderr)
    );
    let launcher_name = if cfg!(windows) { "t3.cmd" } else { "t3" };
    let shim_path = bin_dir.join(launcher_name);
    assert!(shim_path.exists());

    // Kill switches for INSTALL, UNINSTALL, and UP log exact unit names.
    let ks_log = temp.path().join("svc-unit-ks.jsonl");
    let ks_log_str = ks_log.to_string_lossy().into_owned();
    let ks_env = isolated_env(
        temp.path(),
        &[
            ("PATH", &empty_str),
            ("RUST_ANYTHING_FALLBACK_LOG", &ks_log_str),
            ("RUST_ANYTHING_UNIT_UP", "original"),
            ("RUST_ANYTHING_UNIT_INSTALL", "original"),
            ("RUST_ANYTHING_UNIT_UNINSTALL", "original"),
        ],
    );
    assert_eq!(run_t3(&["up", "--no-lan"], &ks_env)?.status.code(), Some(1));
    assert_eq!(run_t3(&["install"], &ks_env)?.status.code(), Some(1));
    assert_eq!(
        run_t3(&["uninstall", "--yes"], &ks_env)?.status.code(),
        Some(1)
    );
    let ks_entries: Vec<Value> = fs::read_to_string(&ks_log)?
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(
        ks_entries
            .iter()
            .map(|v| v["unit"].as_str().unwrap_or(""))
            .collect::<Vec<_>>(),
        vec!["UP", "INSTALL", "UNINSTALL"]
    );

    // Empty home uninstall report matches parity.
    let empty_home = temp.path().join("never-installed");
    let empty_home_str = empty_home.to_string_lossy().into_owned();
    let empty_uninst = run_t3(
        &["uninstall", "--base-dir", &empty_home_str],
        &non_standalone_env,
    )?;
    assert_eq!(empty_uninst.status.code(), Some(0));
    assert!(String::from_utf8(empty_uninst.stdout)?.contains("Nothing to remove:"));

    // Non-TTY uninstall without --yes refuses without deleting.
    let no_yes = run_t3(&["uninstall", "--base-dir", &home_str], &standalone_env)?;
    assert_eq!(no_yes.status.code(), Some(1));
    assert!(shim_path.exists());

    // With --yes, uninstall deletes shim and runtime dir cleanly while keeping userdata.
    let yes_out = run_t3(
        &["uninstall", "--yes", "--base-dir", &home_str],
        &standalone_env,
    )?;
    assert_eq!(yes_out.status.code(), Some(0));
    let yes_stdout = String::from_utf8(yes_out.stdout)?;
    assert!(yes_stdout.contains("t3 is uninstalled. Thanks for trying T3 Code."));
    assert!(!shim_path.exists());
    assert!(!home.join("runtime").exists());
    Ok(())
}

#[test]
fn update_validates_channels_policies_downgrades_and_kill_switch() -> TestResult {
    let temp = tempfile::tempdir()?;
    let env = isolated_env(temp.path(), &[("T3_BIN_TS", NONEXISTENT_BIN_TS)]);

    let bad_ch = run_t3(&["update", "--channel", "canary"], &env)?;
    assert_eq!(bad_ch.status.code(), Some(1));
    assert!(String::from_utf8(bad_ch.stderr)?.contains("Invalid value for flag --channel"));

    // Managed policy disabling updates stops `t3 update`.
    let policy_file = temp.path().join("policy.json");
    let policy_doc = json!({
        "schemaVersion": 1,
        "organization": "Acme Corp",
        "limits": {
            "updates": {
                "mode": "off"
            }
        }
    });
    fs::write(&policy_file, serde_json::to_vec(&policy_doc)?)?;
    let pol_str = policy_file.to_string_lossy().into_owned();
    let pol_env = isolated_env(
        temp.path(),
        &[
            ("T3_BIN_TS", NONEXISTENT_BIN_TS),
            ("T3CODE_MANAGED_POLICY_FILE", &pol_str),
        ],
    );
    let pol_blocked = run_t3(&["update", "9.9.9"], &pol_env)?;
    assert_eq!(pol_blocked.status.code(), Some(1));
    assert!(
        String::from_utf8(pol_blocked.stderr)?
            .contains("Managed updates are disabled by Acme Corp")
    );

    // Same version exits 0 immediately without network.
    let same = run_t3(&["update", env!("CARGO_PKG_VERSION")], &env)?;
    assert_eq!(same.status.code(), Some(0));
    assert!(String::from_utf8(same.stdout)?.contains("already up to date"));

    // Downgrade without `--allow-downgrade` fails offline.
    let down = run_t3(&["update", "0.0.1"], &env)?;
    assert_eq!(down.status.code(), Some(1));
    assert!(String::from_utf8(down.stderr)?.contains("--allow-downgrade"));

    // Track switch requires `--channel`.
    let cross = run_t3(&["update", "0.2.0-nightly.20260101.1"], &env)?;
    assert_eq!(cross.status.code(), Some(1));
    assert!(String::from_utf8(cross.stderr)?.contains("Pass --channel nightly"));

    // Kill switch logs UPDATE fallback.
    let log_file = temp.path().join("upd-ks.jsonl");
    let log_str = log_file.to_string_lossy().into_owned();
    let empty_bin = temp.path().join("empty-bin");
    fs::create_dir_all(&empty_bin)?;
    let empty_str = empty_bin.to_string_lossy().into_owned();
    let ks_env = isolated_env(
        temp.path(),
        &[
            ("PATH", &empty_str),
            ("RUST_ANYTHING_UNIT_UPDATE", "original"),
            ("RUST_ANYTHING_FALLBACK_LOG", &log_str),
        ],
    );
    assert_eq!(run_t3(&["update"], &ks_env)?.status.code(), Some(1));
    let log: Value = serde_json::from_str(fs::read_to_string(&log_file)?.trim())?;
    assert_eq!(log["unit"], "UPDATE");
    let _ = json!(null);
    Ok(())
}
