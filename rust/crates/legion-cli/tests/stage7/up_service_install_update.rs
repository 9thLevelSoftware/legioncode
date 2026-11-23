//! Offline & sandboxed tests for `legioncode up`, `legioncode service` same-home operations,
//! `legioncode install`, `legioncode uninstall`, and `legioncode update`.

use std::fs;
use std::path::Path;

use legion_persistence::service_launcher::{ServiceState, service_state_path, write_service_state};
use serde_json::{Value, json};

use crate::support::{TestResult, isolated_env, run_legioncode};

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
    let unit_path = temp.path().join("sandbox-unit").join("legioncode-unit");
    let home = temp.path().join("up-home");
    let home_str = home.to_string_lossy().into_owned();
    let unit_str = unit_path.to_string_lossy().into_owned();
    let path_str = fake_bin.to_string_lossy().into_owned();

    let env = isolated_env(
        temp.path(),
        &[
            ("PATH", &path_str),
            ("LEGIONCODE_BIN_TS", NONEXISTENT_BIN_TS),
            ("LEGIONCODE_BOOT_SERVICE_UNIT_PATH", &unit_str),
        ],
    );

    // Non-TTY `legioncode up` without access flag rejects.
    let no_tty = run_legioncode(&["up", "--base-dir", &home_str], &env)?;
    assert_eq!(no_tty.status.code(), Some(1));
    assert!(String::from_utf8(no_tty.stderr)?.contains("Choose access with --lan"));

    // `--connect` rejected cleanly when public config missing.
    let no_connect = run_legioncode(&["up", "--connect", "--base-dir", &home_str], &env)?;
    assert_eq!(no_connect.status.code(), Some(1));
    assert!(String::from_utf8(no_connect.stderr)?.contains("Legion Connect is unavailable"));

    // `--no-lan` when not installed updates `service.env` and skips service install.
    let off_out = run_legioncode(&["up", "--no-lan", "--base-dir", &home_str], &env)?;
    assert_eq!(off_out.status.code(), Some(0));
    assert!(String::from_utf8(off_out.stdout)?.contains("Remote access is off."));
    assert!(fs::read_to_string(home.join("service.env"))?.contains("LEGIONCODE_LAN=false"));
    assert!(!unit_path.exists());

    // `--lan` writes `LEGIONCODE_LAN=true` and installs sandboxed unit + service state.
    let lan_out = run_legioncode(&["up", "--lan", "--base-dir", &home_str], &env)?;
    assert_eq!(lan_out.status.code(), Some(0));
    let lan_stdout = String::from_utf8(lan_out.stdout)?;
    assert!(lan_stdout.contains("Service installed"));
    assert!(lan_stdout.contains("pairing code only"));
    assert!(fs::read_to_string(home.join("service.env"))?.contains("LEGIONCODE_LAN=true"));
    assert!(unit_path.exists());

    // Same-home `service status`, `service update`, `service restart`, `service uninstall`.
    let st_out = run_legioncode(&["service", "status", "--base-dir", &home_str], &env)?;
    assert_eq!(st_out.status.code(), Some(0));
    assert!(String::from_utf8(st_out.stdout)?.contains("LegionCode service"));

    let upd_out = run_legioncode(&["service", "update", "--base-dir", &home_str], &env)?;
    assert_eq!(upd_out.status.code(), Some(0));
    assert!(
        String::from_utf8(upd_out.stdout)?.contains("`legioncode service update` is deprecated")
    );

    let rst_out = run_legioncode(&["service", "restart", "--base-dir", &home_str], &env)?;
    assert_eq!(rst_out.status.code(), Some(0));
    assert!(String::from_utf8(rst_out.stdout)?.contains("Restarted the LegionCode service"));

    // Downgrade protection on `service install`.
    write_service_state(
        &service_state_path(&home),
        &ServiceState {
            protocol: 4,
            active_version: "99.0.0".to_owned(),
            update: None,
        },
    )?;
    let blocked = run_legioncode(&["service", "install", "--base-dir", &home_str], &env)?;
    assert_eq!(blocked.status.code(), Some(1));
    assert!(String::from_utf8(blocked.stderr)?.contains("--allow-downgrade"));

    let forced = run_legioncode(
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

    let rm_out = run_legioncode(&["service", "uninstall", "--base-dir", &home_str], &env)?;
    assert_eq!(rm_out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(rm_out.stdout)?,
        "Removed the LegionCode service.\n"
    );
    assert!(!unit_path.exists());
    Ok(())
}

fn assert_unit_kill_switches(root: &std::path::Path, empty_str: &str) -> TestResult {
    let ks_log = root.join("svc-unit-ks.jsonl");
    let ks_log_str = ks_log.to_string_lossy().into_owned();
    let ks_env = isolated_env(
        root,
        &[
            ("PATH", empty_str),
            ("RUST_ANYTHING_FALLBACK_LOG", &ks_log_str),
            ("RUST_ANYTHING_UNIT_UP", "original"),
            ("RUST_ANYTHING_UNIT_INSTALL", "original"),
            ("RUST_ANYTHING_UNIT_UNINSTALL", "original"),
        ],
    );
    assert_eq!(
        run_legioncode(&["up", "--no-lan"], &ks_env)?.status.code(),
        Some(1)
    );
    assert_eq!(
        run_legioncode(&["install"], &ks_env)?.status.code(),
        Some(1)
    );
    assert_eq!(
        run_legioncode(&["uninstall", "--yes"], &ks_env)?
            .status
            .code(),
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
            ("LEGIONCODE_BIN_TS", NONEXISTENT_BIN_TS),
            ("LEGIONCODE_INSTALL_BIN_DIR", &bin_str),
            ("LEGIONCODE_CLI_PATH", env!("CARGO_BIN_EXE_legioncode")),
        ],
    );
    let err_out = run_legioncode(&["install"], &non_standalone_env)?;
    assert_eq!(err_out.status.code(), Some(1));
    assert!(
        String::from_utf8(err_out.stderr)?.contains("Only the standalone legioncode can install")
    );

    // 2. Standalone runtime installs shim, records launcher, and refuses foreign binary.
    let home = temp.path().join("standalone-home");
    let exe_name = if cfg!(windows) {
        "legioncode.exe"
    } else {
        "legioncode"
    };
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
            ("LEGIONCODE_BIN_TS", NONEXISTENT_BIN_TS),
            ("LEGIONCODE_INSTALL_BIN_DIR", &bin_str),
            ("LEGIONCODE_CLI_PATH", &exe_str),
        ],
    );
    let inst_out = run_legioncode(&["install"], &standalone_env)?;
    assert_eq!(
        inst_out.status.code(),
        Some(0),
        "install stderr: {}",
        String::from_utf8_lossy(&inst_out.stderr)
    );
    let launcher_name = if cfg!(windows) {
        "legioncode.cmd"
    } else {
        "legioncode"
    };
    let shim_path = bin_dir.join(launcher_name);
    assert!(shim_path.exists());

    assert_unit_kill_switches(temp.path(), &empty_str)?;

    // Empty home uninstall report matches parity.
    let empty_home = temp.path().join("never-installed");
    let empty_home_str = empty_home.to_string_lossy().into_owned();
    let empty_uninst = run_legioncode(
        &["uninstall", "--base-dir", &empty_home_str],
        &non_standalone_env,
    )?;
    assert_eq!(empty_uninst.status.code(), Some(0));
    assert!(String::from_utf8(empty_uninst.stdout)?.contains("Nothing to remove:"));

    // Non-TTY uninstall without --yes refuses without deleting.
    let no_yes = run_legioncode(&["uninstall", "--base-dir", &home_str], &standalone_env)?;
    assert_eq!(no_yes.status.code(), Some(1));
    assert!(shim_path.exists());

    // With --yes, uninstall deletes shim and runtime dir cleanly while keeping userdata.
    let yes_out = run_legioncode(
        &["uninstall", "--yes", "--base-dir", &home_str],
        &standalone_env,
    )?;
    assert_eq!(yes_out.status.code(), Some(0));
    let yes_stdout = String::from_utf8(yes_out.stdout)?;
    assert!(yes_stdout.contains("legioncode is uninstalled. Thanks for trying LegionCode."));
    assert!(!shim_path.exists());
    assert!(!home.join("runtime").exists());
    Ok(())
}

#[test]
fn update_validates_channels_policies_downgrades_and_kill_switch() -> TestResult {
    let temp = tempfile::tempdir()?;
    let env = isolated_env(temp.path(), &[("LEGIONCODE_BIN_TS", NONEXISTENT_BIN_TS)]);

    let bad_ch = run_legioncode(&["update", "--channel", "canary"], &env)?;
    assert_eq!(bad_ch.status.code(), Some(1));
    assert!(String::from_utf8(bad_ch.stderr)?.contains("Invalid value for flag --channel"));

    // Managed policy disabling updates stops `legioncode update`.
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
            ("LEGIONCODE_BIN_TS", NONEXISTENT_BIN_TS),
            ("LEGIONCODE_MANAGED_POLICY_FILE", &pol_str),
        ],
    );
    let pol_blocked = run_legioncode(&["update", "9.9.9"], &pol_env)?;
    assert_eq!(pol_blocked.status.code(), Some(1));
    assert!(
        String::from_utf8(pol_blocked.stderr)?
            .contains("Managed updates are disabled by Acme Corp")
    );

    // Same version exits 0 immediately without network.
    let same = run_legioncode(&["update", env!("CARGO_PKG_VERSION")], &env)?;
    assert_eq!(same.status.code(), Some(0));
    assert!(String::from_utf8(same.stdout)?.contains("already up to date"));

    // Downgrade without `--allow-downgrade` fails offline.
    let down = run_legioncode(&["update", "0.0.1"], &env)?;
    assert_eq!(down.status.code(), Some(1));
    assert!(String::from_utf8(down.stderr)?.contains("--allow-downgrade"));

    // Track switch requires `--channel`.
    let cross = run_legioncode(&["update", "0.2.0-nightly.20260101.1"], &env)?;
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
    assert_eq!(run_legioncode(&["update"], &ks_env)?.status.code(), Some(1));
    let log: Value = serde_json::from_str(fs::read_to_string(&log_file)?.trim())?;
    assert_eq!(log["unit"], "UPDATE");
    let _ = json!(null);
    Ok(())
}
