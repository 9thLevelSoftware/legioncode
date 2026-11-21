//! Integration tests for `t3 __service-preflight` and human `t3 status` parity vs Node.

mod support;

use std::fs;
use std::path::Path;
use std::process::Command;

use serde_json::Value;
use t3_cli::invocation::{powershell_word, shell_word};
use t3_persistence::service_preflight::V2_MIGRATION_MANIFEST;
use t3_persistence::sqlite::SqliteStore;

use support::{TestResult, isolated_env, run_t3};

fn prepended_path_env(bin_dir: &Path) -> String {
    let existing = std::env::var("PATH").unwrap_or_default();
    let sep = if cfg!(windows) { ";" } else { ":" };
    if existing.is_empty() {
        bin_dir.to_string_lossy().into_owned()
    } else {
        format!("{}{sep}{existing}", bin_dir.display())
    }
}

fn create_dummy_t3_on_path(bin_dir: &Path) -> TestResult {
    fs::create_dir_all(bin_dir)?;
    #[cfg(windows)]
    {
        fs::write(bin_dir.join("t3.cmd"), "@echo off\r\nexit /b 0\r\n")?;
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let path = bin_dir.join("t3");
        fs::write(&path, "#!/bin/sh\nexit 0\n")?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

fn run_node_status(
    args: &[&str],
    envs: &[(&str, &str)],
) -> Result<String, Box<dyn std::error::Error>> {
    let mut cmd = Command::new("node");
    cmd.arg("d:/t3code/apps/server/src/bin.ts");
    cmd.arg("status");
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
            "node status failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        )
        .into());
    }
    Ok(String::from_utf8(out.stdout)?)
}

#[test]
fn service_preflight_checks_protocol_and_migrations() -> TestResult {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let env = isolated_env(temp.path(), &[]);

    let missing_db = home.join("missing.sqlite");
    let out_ready_missing = run_t3(
        &[
            "__service-preflight",
            "--database-path",
            &missing_db.to_string_lossy(),
            "--launcher-protocol",
            "4",
        ],
        &env,
    )?;
    assert!(out_ready_missing.status.success());
    let ready_missing: Value = serde_json::from_slice(&out_ready_missing.stdout)?;
    assert_eq!(ready_missing["status"], "ready");
    assert_eq!(ready_missing["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(ready_missing["launcherProtocol"], 4);

    let out_old_proto = run_t3(
        &[
            "__service-preflight",
            "--database-path",
            &missing_db.to_string_lossy(),
            "--launcher-protocol",
            "1",
        ],
        &env,
    )?;
    assert!(out_old_proto.status.success());
    let blocked_proto: Value = serde_json::from_slice(&out_old_proto.stdout)?;
    assert_eq!(blocked_proto["status"], "blocked");
    assert!(
        blocked_proto["reason"]
            .as_str()
            .is_some_and(|r| r.contains("protocol 1") && r.contains("protocol 4"))
    );

    let valid_db = home.join("valid.sqlite");
    let store = SqliteStore::open(&valid_db)?;
    store
        .connection()
        .execute(
            "CREATE TABLE IF NOT EXISTS effect_sql_migrations (migration_id INTEGER PRIMARY KEY, created_at TEXT, name TEXT)",
            [],
        )?;
    for &(migration_id, name) in V2_MIGRATION_MANIFEST {
        store.connection().execute(
            "INSERT OR REPLACE INTO effect_sql_migrations (migration_id, created_at, name) VALUES (?1, '2026-11-20', ?2)",
            rusqlite::params![migration_id, name],
        )?;
    }
    drop(store);

    let out_valid = run_t3(
        &[
            "__service-preflight",
            "--database-path",
            &valid_db.to_string_lossy(),
            "--launcher-protocol",
            "3",
        ],
        &env,
    )?;
    assert!(out_valid.status.success());
    let valid_json: Value = serde_json::from_slice(&out_valid.stdout)?;
    assert_eq!(valid_json["status"], "ready");
    assert_eq!(valid_json["launcherProtocol"], 3);

    let corrupt_db = home.join("corrupt.sqlite");
    fs::write(&corrupt_db, b"not-a-sqlite-database")?;
    let out_corrupt = run_t3(
        &[
            "__service-preflight",
            "--database-path",
            &corrupt_db.to_string_lossy(),
            "--launcher-protocol",
            "4",
        ],
        &env,
    )?;
    assert!(out_corrupt.status.success());
    let corrupt_json: Value = serde_json::from_slice(&out_corrupt.stdout)?;
    assert_eq!(corrupt_json["status"], "blocked");
    Ok(())
}

#[test]
fn service_preflight_honors_kill_switch_and_help_delegation() -> TestResult {
    let temp = tempfile::tempdir()?;
    let log_file = temp.path().join("fallback.jsonl");
    let log_str = log_file.to_string_lossy().into_owned();
    let db_path = temp.path().join("statev2.sqlite");
    let db_str = db_path.to_string_lossy().into_owned();
    let env = isolated_env(
        temp.path(),
        &[
            ("RUST_ANYTHING_UNIT_SERVICE_PREFLIGHT", "original"),
            ("RUST_ANYTHING_FALLBACK_LOG", &log_str),
        ],
    );
    let out = run_t3(
        &[
            "__service-preflight",
            "--database-path",
            &db_str,
            "--launcher-protocol",
            "4",
        ],
        &env,
    )?;
    assert!(out.status.success());
    let parsed: Value = serde_json::from_slice(&out.stdout)?;
    assert_eq!(parsed["status"], "ready");
    let logged = fs::read_to_string(&log_file)?;
    assert!(logged.contains(r#""unit":"SERVICE_PREFLIGHT""#));
    Ok(())
}

#[test]
fn human_status_matches_node_on_empty_home_and_host_override() -> TestResult {
    let temp = tempfile::tempdir()?;
    let base_dir = temp.path().join("status-home");
    fs::create_dir_all(&base_dir)?;
    let bin_dir = temp.path().join("bin");
    create_dummy_t3_on_path(&bin_dir)?;
    let path_env = prepended_path_env(&bin_dir);
    let base_str = base_dir.to_string_lossy().into_owned();
    let env = isolated_env(temp.path(), &[("PATH", &path_env)]);

    let rust_empty = run_t3(&["status", "--base-dir", &base_str], &env)?;
    assert!(rust_empty.status.success());
    let rust_empty_stdout = String::from_utf8(rust_empty.stdout)?;
    let node_empty_stdout = run_node_status(&["--base-dir", &base_str], &env)?;
    assert_eq!(rust_empty_stdout, node_empty_stdout);
    assert!(rust_empty_stdout.contains("Service        not installed"));
    assert!(rust_empty_stdout.contains("Local network  off"));

    fs::write(base_dir.join("service.env"), "T3CODE_HOST=10.0.0.5\n")?;
    let rust_host = run_t3(&["status", "--base-dir", &base_str], &env)?;
    assert!(rust_host.status.success());
    let rust_host_stdout = String::from_utf8(rust_host.stdout)?;
    let node_host_stdout = run_node_status(&["--base-dir", &base_str], &env)?;
    assert_eq!(rust_host_stdout, node_host_stdout);
    assert!(rust_host_stdout.contains("overridden \u{00B7} T3CODE_HOST=10.0.0.5"));

    fs::write(base_dir.join("service.env"), "T3CODE_LAN=true\n")?;
    let rust_lan = run_t3(&["status", "--base-dir", &base_str], &env)?;
    assert!(rust_lan.status.success());
    let rust_lan_stdout = String::from_utf8(rust_lan.stdout)?;
    let node_lan_stdout = run_node_status(&["--base-dir", &base_str], &env)?;
    assert_eq!(rust_lan_stdout, node_lan_stdout);
    assert!(rust_lan_stdout.contains("Anyone on your LAN or tailnet"));
    Ok(())
}

#[test]
fn human_status_quotes_launcher_fallback_and_delegates_when_cloud_configured() -> TestResult {
    assert_eq!(shell_word("plain-flag_1"), "plain-flag_1");
    assert_eq!(shell_word("two words's"), r#"'two words'"'"'s'"#);
    assert_eq!(powershell_word("--base-dir"), "\"--base-dir\"");
    assert_eq!(powershell_word("$`\"a"), "\"`$``\"\"a\"");

    let temp = tempfile::tempdir()?;
    let base_dir = temp.path().join("no-path-home");
    fs::create_dir_all(&base_dir)?;
    let base_str = base_dir.to_string_lossy().into_owned();

    // Without `t3` on PATH, human status uses `& "<exe>"` on Windows or `<exe>` on POSIX.
    let no_t3_dir = temp.path().join("empty-path");
    fs::create_dir_all(&no_t3_dir)?;
    let empty_path_str = no_t3_dir.to_string_lossy().into_owned();
    let no_path_env = isolated_env(temp.path(), &[("PATH", &empty_path_str)]);
    let out_fallback_exe = run_t3(&["status", "--base-dir", &base_str], &no_path_env)?;
    assert!(out_fallback_exe.status.success());
    let stdout_exe = String::from_utf8(out_fallback_exe.stdout)?;
    #[cfg(windows)]
    assert!(stdout_exe.contains("Change access: `& \""));
    #[cfg(not(windows))]
    assert!(stdout_exe.contains("Change access: `"));

    // Cloud config keeps both `--json` and human `t3 status` completely native (zero fallback).
    let log_file = temp.path().join("status-cloud-fallback.jsonl");
    let log_str = log_file.to_string_lossy().into_owned();
    let cloud_env = isolated_env(
        temp.path(),
        &[
            ("T3CODE_CLOUD_PUBLIC_CONFIG", "1"),
            ("RUST_ANYTHING_FALLBACK_LOG", &log_str),
            ("T3_BIN_TS", r"Z:\nonexistent\bin.ts"),
        ],
    );
    let out_json = run_t3(&["status", "--json", "--base-dir", &base_str], &cloud_env)?;
    assert!(out_json.status.success());
    assert!(!log_file.exists());

    let out_human_cloud = run_t3(&["status", "--base-dir", &base_str], &cloud_env)?;
    assert!(out_human_cloud.status.success());
    let human_stdout = String::from_utf8(out_human_cloud.stdout)?;
    assert!(human_stdout.contains("T3 Connect"));
    assert!(!log_file.exists());

    let ks_env = isolated_env(
        temp.path(),
        &[
            ("RUST_ANYTHING_UNIT_STATUS", "original"),
            ("RUST_ANYTHING_FALLBACK_LOG", &log_str),
            ("PATH", &empty_path_str),
        ],
    );
    assert_eq!(
        run_t3(&["status", "--base-dir", &base_str], &ks_env)?
            .status
            .code(),
        Some(1)
    );
    assert!(fs::read_to_string(&log_file)?.contains(r#""unit":"STATUS""#));
    Ok(())
}
