//! Tests for `t3 app` (with live `DesktopControlServer`), `t3 triage`, and `t3 browser setup`.

use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde_json::Value;
use t3_desktop_shell::control_server::{
    DesktopControlRequest, DesktopControlResponse, DesktopControlServer,
};
use t3_shared::desktop_app_control::{DesktopControlPlatform, resolve_desktop_app_control_address};

use crate::support::{TestResult, isolated_env, run_t3};

const NONEXISTENT_BIN_TS: &str = r"Z:\nonexistent\bin.ts";

#[tokio::test]
async fn app_command_guards_ssh_and_activates_live_desktop_control_server() -> TestResult {
    let temp = tempfile::tempdir()?;
    let base_dir = temp.path().join("app-home");
    let workspace = temp.path().join("workspace-proj");
    fs::create_dir_all(&workspace)?;
    let base_str = base_dir.to_string_lossy().into_owned();
    let ws_str = workspace.to_string_lossy().into_owned();

    // 1. SSH rejection guard.
    let ssh_env = isolated_env(
        temp.path(),
        &[("T3_BIN_TS", NONEXISTENT_BIN_TS), ("SSH_TTY", "/dev/pts/1")],
    );
    let ssh_out = run_t3(&["app", &ws_str, "--base-dir", &base_str], &ssh_env)?;
    assert_eq!(ssh_out.status.code(), Some(1));
    assert!(String::from_utf8(ssh_out.stderr)?.contains("cannot run over SSH"));

    // 2. Live DesktopControlServer bound at the platform control address for base_dir/userdata.
    let state_dir = base_dir.join("userdata");
    let uid = current_uid_for_test();
    let temp_dir = std::env::temp_dir().to_string_lossy().into_owned();
    let addr = resolve_desktop_app_control_address(
        &state_dir.to_string_lossy(),
        DesktopControlPlatform::current(),
        &temp_dir,
        uid,
    );
    if let Some(ref dir) = addr.directory {
        fs::create_dir_all(dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        }
    }

    let seen: Arc<Mutex<Vec<DesktopControlRequest>>> = Arc::new(Mutex::new(Vec::new()));
    let fail_mode = Arc::new(Mutex::new(false));
    let seen_cb = Arc::clone(&seen);
    let fail_cb = Arc::clone(&fail_mode);
    let handler = Arc::new(move |req: DesktopControlRequest| {
        if let Ok(mut guard) = seen_cb.lock() {
            guard.push(req.clone());
        }
        if fail_cb.lock().is_ok_and(|g| *g) {
            DesktopControlResponse::error(req.request_id, "project-create-failed", "denied")
        } else {
            DesktopControlResponse::activated(req.request_id, req.workspace_root)
        }
    });

    #[cfg(windows)]
    let server = DesktopControlServer::bind_named_pipe(addr.address.clone(), handler)?;
    #[cfg(unix)]
    let server = DesktopControlServer::bind_unix_socket(
        addr.address.clone(),
        addr.directory.as_deref(),
        handler,
    )?;

    let home_path = temp.path().to_path_buf();
    let ws_arg = ws_str.clone();
    let base_arg = base_str.clone();
    let ok_out = tokio::task::spawn_blocking(move || {
        let env_ok = isolated_env(&home_path, &[("T3_BIN_TS", NONEXISTENT_BIN_TS)]);
        run_t3(&["app", &ws_arg, "--base-dir", &base_arg], &env_ok)
    })
    .await??;
    assert_eq!(
        ok_out.status.code(),
        Some(0),
        "app stderr: {}",
        String::from_utf8_lossy(&ok_out.stderr)
    );
    let stdout = String::from_utf8(ok_out.stdout)?;
    assert!(stdout.starts_with("Opened "), "stdout={stdout:?}");

    {
        let recorded = seen.lock().map_err(|_| "poisoned mutex")?;
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].version, 1);
        assert_eq!(recorded[0].request_type.as_deref(), Some("open-workspace"));
        assert_ne!(recorded[0].workspace_root, "");
    }

    // 3. Explicit error response bubbles request-failed error.
    if let Ok(mut guard) = fail_mode.lock() {
        *guard = true;
    }
    let home_path = temp.path().to_path_buf();
    let ws_arg = ws_str.clone();
    let base_arg = base_str.clone();
    let err_out = tokio::task::spawn_blocking(move || {
        let env_err = isolated_env(&home_path, &[("T3_BIN_TS", NONEXISTENT_BIN_TS)]);
        run_t3(&["app", &ws_arg, "--base-dir", &base_arg], &env_err)
    })
    .await??;
    assert_eq!(err_out.status.code(), Some(1));
    assert!(String::from_utf8(err_out.stderr)?.contains("project-create-failed"));

    server.shutdown().await;

    let miss_out = run_t3(
        &["app", &ws_str, "--base-dir", &base_str],
        &isolated_env(temp.path(), &[("T3_BIN_TS", NONEXISTENT_BIN_TS)]),
    )?;
    assert_eq!(miss_out.status.code(), Some(1));
    assert!(String::from_utf8(miss_out.stderr)?.contains("DesktopAppUnreachableError"));
    Ok(())
}

#[tokio::test]
async fn app_command_finds_implicit_dev_socket_when_userdata_absent() -> TestResult {
    let temp = tempfile::tempdir()?;
    let implicit_root = temp.path().join("implicit-user");
    let workspace = temp.path().join("workspace-dev");
    fs::create_dir_all(&workspace)?;
    let ws_dev = workspace.to_string_lossy().into_owned();
    let dev_state_dir = implicit_root.join(".t3").join("dev");
    let uid = current_uid_for_test();
    let temp_dir = std::env::temp_dir().to_string_lossy().into_owned();
    let dev_addr = resolve_desktop_app_control_address(
        &dev_state_dir.to_string_lossy(),
        DesktopControlPlatform::current(),
        &temp_dir,
        uid,
    );
    if let Some(ref dir) = dev_addr.directory {
        fs::create_dir_all(dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        }
    }
    let dev_seen: Arc<Mutex<Vec<DesktopControlRequest>>> = Arc::new(Mutex::new(Vec::new()));
    let dev_seen_cb = Arc::clone(&dev_seen);
    let dev_handler = Arc::new(move |req: DesktopControlRequest| {
        if let Ok(mut guard) = dev_seen_cb.lock() {
            guard.push(req.clone());
        }
        DesktopControlResponse::activated(req.request_id, req.workspace_root)
    });
    #[cfg(windows)]
    let dev_server = DesktopControlServer::bind_named_pipe(dev_addr.address.clone(), dev_handler)?;
    #[cfg(unix)]
    let dev_server = DesktopControlServer::bind_unix_socket(
        dev_addr.address.clone(),
        dev_addr.directory.as_deref(),
        dev_handler,
    )?;
    let implicit_clone = implicit_root.clone();
    let dev_out = tokio::task::spawn_blocking(move || {
        let env_dev = isolated_env(&implicit_clone, &[("T3_BIN_TS", NONEXISTENT_BIN_TS)]);
        run_t3(&["app", &ws_dev], &env_dev)
    })
    .await??;
    assert_eq!(dev_out.status.code(), Some(0));
    dev_server.shutdown().await;
    assert_eq!(dev_seen.lock().map_err(|_| "poisoned")?.len(), 1);
    Ok(())
}

#[test]
fn triage_creates_context_and_launches_stub_agent_offline() -> TestResult {
    let temp = tempfile::tempdir()?;
    let empty_bin = temp.path().join("empty-bin");
    fs::create_dir_all(&empty_bin)?;
    let empty_str = empty_bin.to_string_lossy().into_owned();
    let base_dir = temp.path().join("tri-home");
    let base_str = base_dir.to_string_lossy().into_owned();

    // 1. No agents found writes playbook & context and exits 0.
    let no_agent_env = isolated_env(
        temp.path(),
        &[("PATH", &empty_str), ("T3_BIN_TS", NONEXISTENT_BIN_TS)],
    );
    let no_agent_out = run_t3(&["triage", "--base-dir", &base_str], &no_agent_env)?;
    assert_eq!(no_agent_out.status.code(), Some(0));
    let out_text = String::from_utf8(no_agent_out.stdout)?;
    assert!(out_text.contains("No supported agent CLI (claude, codex, agy) was found"));
    assert!(out_text.contains("triage-playbook.md"));
    assert!(out_text.contains("context.md"));

    // 2. Requested missing agent and invalid agent fail cleanly.
    let out_missing = run_t3(
        &["triage", "--agent", "codex", "--base-dir", &base_str],
        &no_agent_env,
    )?;
    assert_eq!(out_missing.status.code(), Some(1));
    assert!(String::from_utf8(out_missing.stderr)?.contains("TriageAgentUnavailableError"));

    let out_invalid = run_t3(
        &["triage", "--agent", "bogus", "--base-dir", &base_str],
        &no_agent_env,
    )?;
    assert_eq!(out_invalid.status.code(), Some(1));
    assert!(String::from_utf8(out_invalid.stderr)?.contains("Invalid value for flag --agent"));

    // 3. Single stub agent on PATH launches cleanly.
    let agent_bin = temp.path().join("agent-bin");
    create_stub_script(&agent_bin, "claude")?;
    let agent_bin_str = agent_bin.to_string_lossy().into_owned();
    let single_agent_env = isolated_env(
        temp.path(),
        &[("PATH", &agent_bin_str), ("T3_BIN_TS", NONEXISTENT_BIN_TS)],
    );
    let single_agent_out = run_t3(&["triage", "--base-dir", &base_str], &single_agent_env)?;
    assert_eq!(single_agent_out.status.code(), Some(0));
    assert!(String::from_utf8(single_agent_out.stdout)?.contains("Starting Claude Code."));

    // 4. Multiple stub agents in non-TTY require --agent flag.
    create_stub_script(&agent_bin, "codex")?;
    let out_multi = run_t3(&["triage", "--base-dir", &base_str], &single_agent_env)?;
    assert_eq!(out_multi.status.code(), Some(1));
    assert!(String::from_utf8(out_multi.stderr)?.contains("TriageAgentChoiceRequiredError"));
    Ok(())
}

#[test]
fn browser_setup_and_kill_switches_work_offline() -> TestResult {
    let temp = tempfile::tempdir()?;
    let base_dir = temp.path().join("br-home");
    let env = isolated_env(temp.path(), &[("T3_BIN_TS", NONEXISTENT_BIN_TS)]);

    let out = run_t3(
        &[
            "browser",
            "setup",
            "--base-dir",
            &base_dir.to_string_lossy(),
        ],
        &env,
    )?;
    assert_eq!(out.status.code(), Some(0));
    #[cfg(not(target_os = "linux"))]
    assert_eq!(
        String::from_utf8(out.stdout)?,
        "Nothing to set up: T3's browser runs as is on this system.\n"
    );

    let log_file = temp.path().join("browser-ks.jsonl");
    let log_str = log_file.to_string_lossy().into_owned();
    let empty_bin = temp.path().join("empty-bin");
    fs::create_dir_all(&empty_bin)?;
    let empty_str = empty_bin.to_string_lossy().into_owned();
    let ks_env = isolated_env(
        temp.path(),
        &[
            ("PATH", &empty_str),
            ("RUST_ANYTHING_UNIT_BROWSER", "original"),
            ("RUST_ANYTHING_FALLBACK_LOG", &log_str),
        ],
    );
    assert_eq!(
        run_t3(&["browser", "setup"], &ks_env)?.status.code(),
        Some(1)
    );
    let log: Value = serde_json::from_str(fs::read_to_string(&log_file)?.trim())?;
    assert_eq!(log["unit"], "BROWSER");
    Ok(())
}

fn create_stub_script(dir: &Path, name: &str) -> TestResult {
    fs::create_dir_all(dir)?;
    #[cfg(windows)]
    fs::write(
        dir.join(format!("{name}.cmd")),
        "@echo off\r\nexit /b 0\r\n",
    )?;
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let p = dir.join(name);
        fs::write(&p, "#!/bin/sh\nexit 0\n")?;
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

fn current_uid_for_test() -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        fs::metadata("/proc/self").ok().map(|m| m.uid())
    }
    #[cfg(not(unix))]
    {
        None
    }
}
