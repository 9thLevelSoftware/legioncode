//! Focused Wave N integration tests for `connect`, `thread` offline parity, `acp-mcp-*`,
//! `__ssh-helper`, `__service-launcher`, `fallback` hardening, and unit kill switches.

mod support;

use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};

use legion_cli::serve::{ServeConfig, ServeMode, spawn_server};
use legion_persistence::service_launcher::{
    ServiceState, ServiceUpdateRecord, service_state_path, write_service_state,
};
use serde_json::{Value, json};

use support::{TestResult, isolated_env, run_legioncode};

const NONEXISTENT_BIN_TS: &str = r"Z:\nonexistent\bin.ts";

#[test]
fn connect_unavailable_matches_node_stdout_stderr_and_honors_kill_switch() -> TestResult {
    let temp = tempfile::tempdir()?;
    let env = isolated_env(temp.path(), &[("LEGIONCODE_BIN_TS", NONEXISTENT_BIN_TS)]);
    let out = run_legioncode(&["connect"], &env)?;
    assert_eq!(out.status.code(), Some(1));

    let stdout = String::from_utf8(out.stdout)?;
    assert!(stdout.starts_with("DESCRIPTION\n  Legion Connect is unavailable in builds without public configuration.\n\nUSAGE\n  legioncode connect [flags] [<command...>]"));
    assert!(stdout.ends_with("none)\n"));
    assert!(!stdout.ends_with("\n\n"));

    let stderr = String::from_utf8(out.stderr)?;
    assert_eq!(
        stderr,
        "\nERROR\n  Legion Connect commands are unavailable: this build is missing Legion Connect public configuration.\n"
    );

    let log_file = temp.path().join("connect-ks.jsonl");
    let log_str = log_file.to_string_lossy().into_owned();
    let ks_env = isolated_env(
        temp.path(),
        &[
            ("RUST_ANYTHING_UNIT_CONNECT", "original"),
            ("RUST_ANYTHING_FALLBACK_LOG", &log_str),
        ],
    );
    let _ = run_legioncode(&["connect"], &ks_env)?;
    assert!(fs::read_to_string(&log_file)?.contains(r#""unit":"CONNECT""#));
    Ok(())
}

#[test]
fn thread_offline_error_stream_and_kill_switch() -> TestResult {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("offline-home");
    fs::create_dir_all(&home)?;
    let home_str = home.to_string_lossy().into_owned();
    let env = isolated_env(temp.path(), &[("LEGIONCODE_BIN_TS", NONEXISTENT_BIN_TS)]);

    let out = run_legioncode(&["thread", "list", "--base-dir", &home_str], &env)?;
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(out.stdout, [] as [u8; 0]);
    assert_eq!(
        String::from_utf8(out.stderr)?,
        format!("No running LegionCode server uses {home_str}. Start LegionCode, then retry.\n")
    );

    let log_file = temp.path().join("thread-ks.jsonl");
    let log_str = log_file.to_string_lossy().into_owned();
    let ks_env = isolated_env(
        temp.path(),
        &[
            ("RUST_ANYTHING_UNIT_THREAD", "original"),
            ("RUST_ANYTHING_FALLBACK_LOG", &log_str),
            ("LEGIONCODE_BIN_TS", NONEXISTENT_BIN_TS),
        ],
    );
    let ks_out = run_legioncode(&["thread", "list", "--base-dir", &home_str], &ks_env)?;
    assert!(!ks_out.status.success());
    assert!(fs::read_to_string(&log_file)?.contains(r#""unit":"THREAD""#));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn acp_mcp_call_and_bridge_talk_to_live_serve_mcp() -> TestResult {
    let temp = tempfile::tempdir()?;
    let env_missing = isolated_env(temp.path(), &[("LEGIONCODE_BIN_TS", NONEXISTENT_BIN_TS)]);
    let missing_out = run_legioncode(&["acp-mcp-call", "legion_project_list", "{}"], &env_missing)?;
    assert_eq!(missing_out.status.code(), Some(2));
    assert_eq!(
        String::from_utf8(missing_out.stderr)?,
        "acp-mcp-call requires LEGIONCODE_ACP_MCP_ENDPOINT and LEGIONCODE_ACP_MCP_AUTHORIZATION.\n"
    );

    let base_dir = temp.path().join("mcp-home");
    let server = spawn_server(ServeConfig {
        base_dir: base_dir.clone(),
        base_dir_is_explicit: true,
        port: 0,
        host: Some("127.0.0.1".to_owned()),
        mode: ServeMode::Web,
        no_browser: true,
        lan: false,
        static_dir: None,
        cwd: temp.path().to_path_buf(),
    })
    .await?;
    // Seed a valid bearer session via pairing endpoint for /mcp tools.
    let now_ms = 1_700_000_000_000_i64;
    let issued = legion_persistence::pairing_tokens::issue_pairing_token(
        &server.paths().db_path,
        &legion_persistence::pairing_tokens::IssuePairingTokenInput::default().with_ttl_ms(60_000),
        now_ms.max(i64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_millis(),
        )?),
    )?;
    let http = reqwest::Client::new();
    let pair_json: Value = http
        .post(format!("{}/api/auth/pair", server.origin()))
        .json(&json!({ "token": issued.credential, "clientLabel": "acp-test" }))
        .send()
        .await?
        .json()
        .await?;
    let auth_hdr = format!(
        "Bearer {}",
        pair_json["accessToken"]
            .as_str()
            .ok_or("missing accessToken")?
    );
    let endpoint = format!("{}/mcp", server.origin());
    let mcp_env = isolated_env(
        temp.path(),
        &[
            ("LEGIONCODE_BIN_TS", NONEXISTENT_BIN_TS),
            ("LEGIONCODE_MCP_ENDPOINT", &endpoint),
            ("LEGIONCODE_MCP_AUTHORIZATION", &auth_hdr),
        ],
    );

    let call_out = run_legioncode(&["acp-mcp-call", "legion_project_list", "{}"], &mcp_env)?;
    assert!(
        call_out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&call_out.stderr)
    );
    let parsed: Value = serde_json::from_slice(&call_out.stdout)?;
    assert_eq!(parsed["isError"], false);
    assert!(parsed["structuredContent"]["projects"].is_array());

    let mut child = Command::new(env!("CARGO_BIN_EXE_legioncode"))
        .arg("acp-mcp-bridge")
        .env("LEGIONCODE_BIN_TS", NONEXISTENT_BIN_TS)
        .env("LEGIONCODE_ACP_MCP_ENDPOINT", &endpoint)
        .env("LEGIONCODE_ACP_MCP_AUTHORIZATION", &auth_hdr)
        .env_remove("RUST_ANYTHING_UNIT_ACP_MCP")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        writeln!(
            stdin,
            "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{{}}}}"
        )?;
        writeln!(
            stdin,
            "{{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}}"
        )?;
        writeln!(
            stdin,
            "{{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\",\"params\":{{}}}}"
        )?;
    }
    let output = child.wait_with_output()?;
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let bridge_out = String::from_utf8(output.stdout)?;
    assert!(bridge_out.contains("legion_project_list"));
    assert!(bridge_out.contains("legion_thread_list"));

    server.shutdown().await?;
    Ok(())
}

#[test]
fn service_launcher_validates_protocol_v4_and_missing_home_without_node() -> TestResult {
    let temp = tempfile::tempdir()?;
    let env_no_home = isolated_env(temp.path(), &[("LEGIONCODE_BIN_TS", NONEXISTENT_BIN_TS)]);
    let no_home_out = run_legioncode(&["__service-launcher"], &env_no_home)?;
    assert_eq!(no_home_out.status.code(), Some(1));
    assert!(String::from_utf8(no_home_out.stderr)?.contains("LEGIONCODE_HOME is required"));

    let base_dir = temp.path().join("svc-home");
    let log_path = temp.path().join("launcher.log");
    let state_path = service_state_path(&base_dir);
    write_service_state(
        &state_path,
        &ServiceState {
            protocol: 4,
            active_version: "0.1.9".to_owned(),
            update: Some(ServiceUpdateRecord::Pending {
                id: "upd-1".to_owned(),
                from_version: "0.1.9".to_owned(),
                target_version: "0.1.10".to_owned(),
                db_path: base_dir
                    .join("userdata/statev2.sqlite")
                    .to_string_lossy()
                    .into_owned(),
            }),
        },
    )?;

    let base_str = base_dir.to_string_lossy().into_owned();
    let log_str = log_path.to_string_lossy().into_owned();
    let blocked = run_legioncode(
        &[
            "__service-launcher",
            "--base-dir",
            &base_str,
            "--log-file",
            &log_str,
        ],
        &env_no_home,
    )?;
    assert_eq!(blocked.status.code(), Some(1));
    assert!(fs::read_to_string(&log_path)?.contains("target-runtime-missing"));
    Ok(())
}

#[test]
fn ssh_helper_pick_port_wait_ready_and_update_remaining_formats() -> TestResult {
    let temp = tempfile::tempdir()?;
    let env = isolated_env(temp.path(), &[("LEGIONCODE_BIN_TS", NONEXISTENT_BIN_TS)]);
    let port_file = temp.path().join("preferred.port");
    let picked = run_legioncode(
        &[
            "__ssh-helper",
            "pick-port",
            &port_file.to_string_lossy(),
            "45100",
            "50",
        ],
        &env,
    )?;
    assert!(picked.status.success());
    let port_text = String::from_utf8(picked.stdout)?;
    assert!(!port_text.ends_with('\n'));
    let chosen: u16 = port_text.parse()?;
    assert!((45100..45150).contains(&chosen));

    let marker = temp.path().join("update-in-progress.json");
    let rem_empty = run_legioncode(
        &[
            "__ssh-helper",
            "update-remaining",
            &marker.to_string_lossy(),
        ],
        &env,
    )?;
    assert!(rem_empty.status.success());
    assert_eq!(rem_empty.stdout, [] as [u8; 0]);

    let now_ms = u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis(),
    )?;
    legion_persistence::service_launcher::publish_update_marker(&marker, now_ms)?;
    let rem_active = run_legioncode(
        &[
            "__ssh-helper",
            "update-remaining",
            &marker.to_string_lossy(),
        ],
        &env,
    )?;
    assert!(rem_active.status.success());
    let rem_text = String::from_utf8(rem_active.stdout)?;
    assert!(!rem_text.ends_with('\n'));
    assert!(rem_text.parse::<u64>()? > 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_project_rename_remove_and_pair_delegations() -> TestResult {
    let temp = tempfile::tempdir()?;
    let base_dir = temp.path().join("proj-ops-home");
    let ws = temp.path().join("ws-ops");
    fs::create_dir_all(&ws)?;

    let server = spawn_server(ServeConfig {
        base_dir: base_dir.clone(),
        base_dir_is_explicit: true,
        port: 0,
        host: Some("127.0.0.1".to_owned()),
        mode: ServeMode::Web,
        no_browser: true,
        lan: false,
        static_dir: None,
        cwd: temp.path().to_path_buf(),
    })
    .await?;

    let env = isolated_env(temp.path(), &[("LEGIONCODE_BIN_TS", NONEXISTENT_BIN_TS)]);
    let base_str = base_dir.to_string_lossy().into_owned();
    let ws_str = ws.to_string_lossy().into_owned();

    assert!(
        run_legioncode(&["project", "add", &ws_str, "--base-dir", &base_str], &env)?
            .status
            .success()
    );
    let ren = run_legioncode(
        &[
            "project",
            "rename",
            &ws_str,
            "Renamed Live",
            "--base-dir",
            &base_str,
        ],
        &env,
    )?;
    assert!(ren.status.success());
    assert!(String::from_utf8(ren.stdout)?.contains("to Renamed Live."));
    let rem = run_legioncode(
        &[
            "project",
            "remove",
            &ws_str,
            "--force",
            "--base-dir",
            &base_str,
        ],
        &env,
    )?;
    assert!(rem.status.success());
    assert!(String::from_utf8(rem.stdout)?.contains("Removed project "));

    server.shutdown().await?;
    Ok(())
}
