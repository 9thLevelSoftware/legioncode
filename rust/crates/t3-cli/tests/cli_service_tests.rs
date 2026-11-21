//! Integration tests for `t3 service` (`RUST_ANYTHING_UNIT_SERVICE`) and `t3 status` edge parity vs Node.

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use t3_edge::boot_service_units::encode_windows_task_utf16le_bom;

use support::{TestResult, isolated_env, run_t3};

fn create_dummy_t3_on_path(bin_dir: &Path) -> TestResult {
    fs::create_dir_all(bin_dir)?;
    #[cfg(windows)]
    fs::write(bin_dir.join("t3.cmd"), "@echo off\r\nexit /b 0\r\n")?;
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let path = bin_dir.join("t3");
        fs::write(&path, "#!/bin/sh\nexit 0\n")?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

fn prepended_path_env(bin_dir: &Path) -> String {
    let existing = std::env::var("PATH").unwrap_or_default();
    let sep = if cfg!(windows) { ";" } else { ":" };
    if existing.is_empty() {
        bin_dir.to_string_lossy().into_owned()
    } else {
        format!("{}{sep}{existing}", bin_dir.display())
    }
}

fn unit_file_path(root: &Path) -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        root.join("t3code").join("t3code-task.xml")
    }
    #[cfg(target_os = "macos")]
    {
        root.join("Library")
            .join("LaunchAgents")
            .join("com.t3tools.t3code.service.plist")
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        root.join("systemd").join("user").join("t3code.service")
    }
}

fn write_mock_service_unit(root: &Path, served_base_dir: &str) -> TestResult {
    let unit_path = unit_file_path(root);
    if let Some(parent) = unit_path.parent() {
        fs::create_dir_all(parent)?;
    }

    #[cfg(target_os = "windows")]
    {
        let xml = format!(
            "<Task><Actions><Exec><Arguments>--headless \"{served_base_dir}\" \"--base-dir\" \"{served_base_dir}\"</Arguments></Exec></Actions></Task>"
        );
        fs::write(unit_path, encode_windows_task_utf16le_bom(&xml))?;
    }
    #[cfg(target_os = "macos")]
    {
        let _ = encode_windows_task_utf16le_bom;
        let plist = format!(
            "<plist><dict><key>EnvironmentVariables</key><dict><key>T3CODE_HOME</key><string>{served_base_dir}</string></dict></dict></plist>"
        );
        fs::write(unit_path, plist)?;
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = encode_windows_task_utf16le_bom;
        let unit = format!("[Service]\nEnvironment=T3CODE_HOME={served_base_dir}\n");
        fs::write(unit_path, unit)?;
    }
    Ok(())
}

fn run_node(args: &[&str], envs: &[(&str, &str)]) -> Result<String, Box<dyn std::error::Error>> {
    let mut cmd = Command::new("node");
    cmd.arg("d:/t3code/apps/server/src/bin.ts");
    cmd.args(args);
    for key in [
        "T3CODE_HOME",
        "T3CODE_HOST",
        "T3CODE_CLOUD_PUBLIC_CONFIG",
        "T3CODE_RELAY_URL",
        "T3CODE_CLERK_PUBLISHABLE_KEY",
        "T3CODE_CLERK_CLI_OAUTH_CLIENT_ID",
        "T3CODE_CLI_PATH",
        "T3CODE_NPM_LAUNCHER_PATH",
    ] {
        cmd.env_remove(key);
    }
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let out = cmd.output()?;
    if !out.status.success() {
        return Err(format!(
            "node {:?} failed ({}): {}",
            args,
            out.status,
            String::from_utf8_lossy(&out.stderr)
        )
        .into());
    }
    Ok(String::from_utf8(out.stdout)?)
}

fn assert_t3_matches_node(
    args: &[&str],
    envs: &[(&str, &str)],
) -> Result<String, Box<dyn std::error::Error>> {
    let rust_out = run_t3(args, envs)?;
    assert!(
        rust_out.status.success(),
        "t3 {:?} failed: {}",
        args,
        String::from_utf8_lossy(&rust_out.stderr)
    );
    let rust_stdout = String::from_utf8(rust_out.stdout)?;
    let node_stdout = run_node(args, envs)?;
    assert_eq!(rust_stdout, node_stdout, "stdout mismatch for {args:?}");
    Ok(rust_stdout)
}

#[test]
fn status_and_service_match_node_when_unsupported_and_not_installed() -> TestResult {
    let temp = tempfile::tempdir()?;
    let bin_dir = temp.path().join("bin");
    create_dummy_t3_on_path(&bin_dir)?;
    let path_env = prepended_path_env(&bin_dir);
    let base_dir = temp.path().join("home-a");
    fs::create_dir_all(&base_dir)?;
    let base_str = base_dir.to_string_lossy().into_owned();

    // Unsupported (`LOCALAPPDATA=""` on Windows, `HOME=""` on Unix)
    let unsupported_env = isolated_env(
        temp.path(),
        &[
            ("PATH", &path_env),
            if cfg!(windows) {
                ("LOCALAPPDATA", "")
            } else {
                ("HOME", "")
            },
        ],
    );
    let status_unavail =
        assert_t3_matches_node(&["status", "--base-dir", &base_str], &unsupported_env)?;
    assert!(status_unavail.contains("Service        unavailable"));

    let status_json_out = assert_t3_matches_node(
        &["status", "--json", "--base-dir", &base_str],
        &unsupported_env,
    )?;
    let parsed_json: Value = serde_json::from_str(&status_json_out)?;
    assert_eq!(parsed_json["service"]["supported"], false);

    let svc_unavail = assert_t3_matches_node(
        &["service", "status", "--base-dir", &base_str],
        &unsupported_env,
    )?;
    assert!(svc_unavail.contains("Status: unavailable on this machine"));

    // Not installed with isolated env
    let not_installed_env = isolated_env(temp.path(), &[("PATH", &path_env)]);
    let svc_missing = assert_t3_matches_node(
        &["service", "status", "--base-dir", &base_str],
        &not_installed_env,
    )?;
    assert!(svc_missing.contains("Status: not installed"));

    let restart_missing = assert_t3_matches_node(
        &["service", "restart", "--base-dir", &base_str],
        &not_installed_env,
    )?;
    assert_eq!(restart_missing, "T3 Code service is not installed.\n");

    let uninstall_missing = assert_t3_matches_node(
        &["service", "uninstall", "--base-dir", &base_str],
        &not_installed_env,
    )?;
    assert_eq!(uninstall_missing, "T3 Code service is not installed.\n");

    if !cfg!(target_os = "linux") {
        let protect_out = assert_t3_matches_node(&["service", "protect"], &not_installed_env)?;
        assert_eq!(
            protect_out,
            "Service protection needs Linux with systemd.\n"
        );
    }
    Ok(())
}

#[test]
fn status_and_service_match_node_when_installed_for_other_base_dir() -> TestResult {
    let temp = tempfile::tempdir()?;
    let bin_dir = temp.path().join("bin");
    create_dummy_t3_on_path(&bin_dir)?;
    let path_env = prepended_path_env(&bin_dir);

    let home_a = temp.path().join("home-a");
    let home_b = temp.path().join("home-b's");
    fs::create_dir_all(&home_a)?;
    fs::create_dir_all(&home_b)?;
    write_mock_service_unit(temp.path(), &home_b.to_string_lossy())?;

    let home_a_str = home_a.to_string_lossy().into_owned();
    let env = isolated_env(temp.path(), &[("PATH", &path_env)]);

    let status_other = assert_t3_matches_node(&["status", "--base-dir", &home_a_str], &env)?;
    assert!(status_other.contains("installed for another T3 home"));
    assert!(!status_other.contains("Local network"));
    assert!(!status_other.contains("Change access:"));

    let status_other_json =
        assert_t3_matches_node(&["status", "--json", "--base-dir", &home_a_str], &env)?;
    let parsed: Value = serde_json::from_str(&status_other_json)?;
    assert_eq!(parsed["service"]["installed"], true);
    assert_eq!(
        parsed["service"]["otherBaseDir"],
        home_b.to_string_lossy().as_ref()
    );

    let svc_status =
        assert_t3_matches_node(&["service", "status", "--base-dir", &home_a_str], &env)?;
    assert!(svc_status.contains("Status: installed for another T3 home"));
    assert!(svc_status.contains("Next: Pass `--base-dir '"));

    let svc_restart =
        assert_t3_matches_node(&["service", "restart", "--base-dir", &home_a_str], &env)?;
    assert!(svc_restart.contains("so it was not restarted. Pass `--base-dir '"));

    let svc_uninstall =
        assert_t3_matches_node(&["service", "uninstall", "--base-dir", &home_a_str], &env)?;
    assert!(svc_uninstall.contains("so it was not removed. Pass `--base-dir '"));
    Ok(())
}

#[test]
fn service_delegates_help_unknown_flags_and_kill_switch_to_node() -> TestResult {
    let temp = tempfile::tempdir()?;
    let empty_bin = temp.path().join("empty-bin");
    fs::create_dir_all(&empty_bin)?;
    let empty_path = empty_bin.to_string_lossy().into_owned();

    let home_a = temp.path().join("home-a");
    fs::create_dir_all(&home_a)?;
    write_mock_service_unit(temp.path(), &home_a.to_string_lossy())?;

    let log_file = temp.path().join("service-fallback.jsonl");
    let log_str = log_file.to_string_lossy().into_owned();
    let home_a_str = home_a.to_string_lossy().into_owned();

    let env = isolated_env(
        temp.path(),
        &[
            ("PATH", &empty_path),
            ("RUST_ANYTHING_FALLBACK_LOG", &log_str),
        ],
    );

    assert_eq!(
        run_t3(&["service", "status", "--help"], &env)?
            .status
            .code(),
        Some(0)
    );
    assert_eq!(
        run_t3(&["service", "install", "--help"], &env)?
            .status
            .code(),
        Some(0)
    );
    assert_eq!(
        run_t3(&["service", "protect", "--extra"], &env)?
            .status
            .code(),
        Some(1)
    );
    assert_eq!(
        run_t3(
            &["service", "unknown-subcmd", "--base-dir", &home_a_str],
            &env
        )?
        .status
        .code(),
        Some(1)
    );
    assert!(
        !log_file.exists(),
        "service help and invalid flags must run natively without Node fallback"
    );

    let env_ks = isolated_env(
        temp.path(),
        &[
            ("PATH", &empty_path),
            ("RUST_ANYTHING_FALLBACK_LOG", &log_str),
            ("RUST_ANYTHING_UNIT_SERVICE", "original"),
        ],
    );
    assert_eq!(
        run_t3(&["service", "status", "--base-dir", &home_a_str], &env_ks)?
            .status
            .code(),
        Some(1)
    );
    assert_eq!(
        run_t3(&["service", "protect"], &env_ks)?.status.code(),
        Some(1)
    );

    let entries: Vec<Value> = fs::read_to_string(&log_file)?
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().all(|e| e["unit"] == "SERVICE"));
    Ok(())
}
