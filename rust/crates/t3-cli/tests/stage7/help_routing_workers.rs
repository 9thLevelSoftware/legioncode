//! Tests for `help`, root `start` / bare-word rejection, `__document-text`, `__claude-history`,
//! and `__managed-chatgpt-token` (`T3_BIN_TS=Z:\nonexistent\bin.ts`).

use std::fs;
use std::process::{Command, Stdio};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::support::{TestResult, isolated_env, run_t3};

const NONEXISTENT_BIN_TS: &str = r"Z:\nonexistent\bin.ts";

#[test]
fn help_and_aliases_match_node_byte_for_byte_offline() -> TestResult {
    let temp = tempfile::tempdir()?;
    let env = isolated_env(temp.path(), &[("T3_BIN_TS", NONEXISTENT_BIN_TS)]);

    let expected_help = include_str!("../../assets/help/root.txt").as_bytes();
    let node_out = Command::new("node")
        .args([t3_cli::fallback::LEGACY_BIN_TS, "help"])
        .output()
        .ok()
        .filter(|out| out.status.success());

    for cmd in ["help", "--help", "-h"] {
        let out = run_t3(&[cmd], &env)?;
        assert_eq!(out.status.code(), Some(0), "cmd={cmd}");
        assert_eq!(
            out.stdout, expected_help,
            "stdout mismatch against root.txt for `t3 {cmd}`"
        );
        if let Some(node) = &node_out {
            assert_eq!(
                out.stdout, node.stdout,
                "stdout mismatch against node for `t3 {cmd}`"
            );
        }
        assert!(out.stderr.is_empty(), "unexpected stderr for `t3 {cmd}`");
    }

    let log_file = temp.path().join("help-fallback.jsonl");
    let log_str = log_file.to_string_lossy().into_owned();
    let empty_bin = temp.path().join("empty-bin");
    fs::create_dir_all(&empty_bin)?;
    let empty_str = empty_bin.to_string_lossy().into_owned();
    let ks_env = isolated_env(
        temp.path(),
        &[
            ("PATH", &empty_str),
            ("RUST_ANYTHING_UNIT_HELP", "original"),
            ("RUST_ANYTHING_FALLBACK_LOG", &log_str),
        ],
    );
    assert_eq!(run_t3(&["help"], &ks_env)?.status.code(), Some(1));
    let log: Value = serde_json::from_str(fs::read_to_string(&log_file)?.trim())?;
    assert_eq!(log["unit"], "HELP");
    Ok(())
}

#[test]
fn root_unknown_bare_word_rejected_and_start_spawns_server() -> TestResult {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let env = isolated_env(temp.path(), &[("T3_BIN_TS", NONEXISTENT_BIN_TS)]);

    // Create a regular file (not directory) in temp and run with temp as relative cwd checks
    let file_word = "not-a-command";
    let out = run_t3(&[file_word], &env)?;
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(out.stdout, [] as [u8; 0]);
    let stderr = String::from_utf8(out.stderr)?;
    assert_eq!(
        stderr,
        "\nERROR\n  Unknown command \"not-a-command\". Use \"t3 --help\" for commands or an explicit path such as \"t3 ./my-project\" for a new directory.\n"
    );
    assert!(!home.exists());

    // Kill switch for CLI_PASSTHROUGH delegates unknown bare words when forced.
    let empty_bin = temp.path().join("empty-bin");
    fs::create_dir_all(&empty_bin)?;
    let empty_str = empty_bin.to_string_lossy().into_owned();
    let log_file = temp.path().join("passthrough.jsonl");
    let log_str = log_file.to_string_lossy().into_owned();
    let ks_env = isolated_env(
        temp.path(),
        &[
            ("PATH", &empty_str),
            ("RUST_ANYTHING_UNIT_CLI_PASSTHROUGH", "original"),
            ("RUST_ANYTHING_FALLBACK_LOG", &log_str),
        ],
    );
    assert_eq!(run_t3(&["bogusword"], &ks_env)?.status.code(), Some(1));
    let ks_entry: Value = serde_json::from_str(fs::read_to_string(&log_file)?.trim())?;
    assert_eq!(ks_entry["unit"], "CLI_PASSTHROUGH");

    // `t3 start` routes to the Tokio HTTP/WS server identically to `t3 serve`.
    let start_home = temp.path().join("start-home");
    let start_home_str = start_home.to_string_lossy().into_owned();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_t3"));
    cmd.args([
        "start",
        "--port",
        "0",
        "--no-browser",
        "--base-dir",
        &start_home_str,
    ])
    .env("T3_BIN_TS", NONEXISTENT_BIN_TS)
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    let mut reader = std::io::BufReader::new(child.stdout.take().ok_or("missing stdout")?);
    let mut line = String::new();
    std::io::BufRead::read_line(&mut reader, &mut line)?;
    let _ = child.kill();
    let _ = child.wait();
    assert!(
        line.contains("T3 Code server listening on http://127.0.0.1:"),
        "unexpected `t3 start` line: {line:?}"
    );
    Ok(())
}

#[test]
fn document_text_worker_extracts_raw_and_preserves_exit_codes() -> TestResult {
    let temp = tempfile::tempdir()?;
    let rtf = temp.path().join("note.rtf");
    fs::write(&rtf, r"{\rtf1\ansi Hello\par World}")?;
    let empty_rtf = temp.path().join("empty.rtf");
    fs::write(&empty_rtf, r"{\rtf1\ansi    }")?;
    let html = temp.path().join("note.html");
    fs::write(&html, "<html><body><p>Hello</p></body></html>")?;

    let env = isolated_env(temp.path(), &[("T3_BIN_TS", NONEXISTENT_BIN_TS)]);
    let rtf_str = rtf.to_string_lossy().into_owned();
    let ok_out = run_t3(&["__document-text", &rtf_str, ".rtf"], &env)?;
    assert_eq!(ok_out.status.code(), Some(0));
    let text = String::from_utf8(ok_out.stdout)?;
    assert_eq!(text, "Hello\nWorld");
    assert!(!text.ends_with('\n'));

    // Terminal document rejections (`.html`/`.doc` unsupported top-level format, empty `.rtf`) exit 2.
    let html_rejected = run_t3(&["__document-text", &html.to_string_lossy(), ".html"], &env)?;
    assert_eq!(html_rejected.status.code(), Some(2));
    let bad_ext = run_t3(&["__document-text", &rtf_str, ".doc"], &env)?;
    assert_eq!(bad_ext.status.code(), Some(2));
    let no_text = run_t3(
        &["__document-text", &empty_rtf.to_string_lossy(), ".rtf"],
        &env,
    )?;
    assert_eq!(no_text.status.code(), Some(2));

    // Retryable OS I/O error exits 1.
    let missing = temp.path().join("missing.rtf");
    let io_err = run_t3(
        &["__document-text", &missing.to_string_lossy(), ".rtf"],
        &env,
    )?;
    assert_eq!(io_err.status.code(), Some(1));

    // Kill switch logs DOCUMENT_TEXT.
    let log_file = temp.path().join("doc-ks.jsonl");
    let log_str = log_file.to_string_lossy().into_owned();
    let empty_bin = temp.path().join("empty-bin");
    fs::create_dir_all(&empty_bin)?;
    let empty_str = empty_bin.to_string_lossy().into_owned();
    let ks_env = isolated_env(
        temp.path(),
        &[
            ("PATH", &empty_str),
            ("RUST_ANYTHING_UNIT_DOCUMENT_TEXT", "original"),
            ("RUST_ANYTHING_FALLBACK_LOG", &log_str),
        ],
    );
    let ks = run_t3(&["__document-text", &rtf_str, ".rtf"], &ks_env)?;
    assert_eq!(ks.status.code(), Some(1));
    let log: Value = serde_json::from_str(fs::read_to_string(&log_file)?.trim())?;
    assert_eq!(log["unit"], "DOCUMENT_TEXT");
    Ok(())
}

#[test]
fn claude_history_worker_reads_and_forks_sessions_offline() -> TestResult {
    let temp = tempfile::tempdir()?;
    let hist_dir = temp.path().join("claude-projects");
    fs::create_dir_all(&hist_dir)?;
    let session_id = "11111111-1111-4111-8111-111111111111";
    let session_file = hist_dir.join(format!("{session_id}.jsonl"));
    let lines = [
        json!({
            "type": "user",
            "uuid": "msg-1",
            "sessionId": session_id,
            "message": { "role": "user", "content": "Hello" }
        })
        .to_string(),
        json!({
            "type": "system",
            "uuid": "msg-2",
            "parentUuid": "msg-1",
            "sessionId": session_id,
            "message": { "role": "system", "content": "sys" }
        })
        .to_string(),
        json!({
            "type": "assistant",
            "uuid": "msg-3",
            "parentUuid": "msg-2",
            "sessionId": session_id,
            "message": { "role": "assistant", "content": "Hi there" }
        })
        .to_string(),
    ];
    fs::write(&session_file, format!("{}\n", lines.join("\n")))?;

    let opts = json!({ "dir": hist_dir }).to_string();
    let env = isolated_env(temp.path(), &[("T3_BIN_TS", NONEXISTENT_BIN_TS)]);

    let list_out = run_t3(
        &["__claude-history", "getSessionMessages", session_id, &opts],
        &env,
    )?;
    assert_eq!(list_out.status.code(), Some(0));
    let messages: Vec<Value> = serde_json::from_slice(&list_out.stdout)?;
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["uuid"], "msg-1");
    assert_eq!(messages[1]["uuid"], "msg-3");

    let fork_out = run_t3(
        &["__claude-history", "forkSession", session_id, &opts],
        &env,
    )?;
    assert_eq!(fork_out.status.code(), Some(0));
    let forked: Value = serde_json::from_slice(&fork_out.stdout)?;
    assert!(forked["sessionId"].is_string());
    assert_ne!(forked["sessionId"], session_id);
    Ok(())
}

fn spawn_mock_managed_token_server(
    server_endpoint: String,
) -> tokio::task::JoinHandle<Result<(), String>> {
    tokio::spawn(async move {
        #[cfg(windows)]
        {
            use tokio::net::windows::named_pipe::ServerOptions;
            let server = ServerOptions::new()
                .first_pipe_instance(true)
                .create(&server_endpoint)
                .map_err(|e| e.to_string())?;
            server.connect().await.map_err(|e| e.to_string())?;
            let (read_half, mut write_half) = tokio::io::split(server);
            let mut reader = BufReader::new(read_half);
            let mut line = String::new();
            reader
                .read_line(&mut line)
                .await
                .map_err(|e| e.to_string())?;
            let req: Value = serde_json::from_str(line.trim()).map_err(|e| e.to_string())?;
            assert_eq!(req["version"], 1);
            assert_eq!(req["instanceId"], "inst-1");
            assert_eq!(req["environmentId"], "env-1");
            write_half
                .write_all(b"{\"token\":\"bare-jwt-token\"}\n")
                .await
                .map_err(|e| e.to_string())?;
            write_half.flush().await.map_err(|e| e.to_string())?;
        }
        #[cfg(unix)]
        {
            use tokio::net::UnixListener;
            let listener = UnixListener::bind(&server_endpoint).map_err(|e| e.to_string())?;
            let (stream, _) = listener.accept().await.map_err(|e| e.to_string())?;
            let (read_half, mut write_half) = stream.into_split();
            let mut reader = BufReader::new(read_half);
            let mut line = String::new();
            reader
                .read_line(&mut line)
                .await
                .map_err(|e| e.to_string())?;
            let req: Value = serde_json::from_str(line.trim()).map_err(|e| e.to_string())?;
            assert_eq!(req["version"], 1);
            assert_eq!(req["instanceId"], "inst-1");
            assert_eq!(req["environmentId"], "env-1");
            write_half
                .write_all(b"{\"token\":\"bare-jwt-token\"}\n")
                .await
                .map_err(|e| e.to_string())?;
            write_half.flush().await.map_err(|e| e.to_string())?;
        }
        Ok::<(), String>(())
    })
}

#[tokio::test]
async fn managed_chatgpt_token_worker_exchanges_frame_and_handles_failures() -> TestResult {
    let temp = tempfile::tempdir()?;
    let env = isolated_env(temp.path(), &[("T3_BIN_TS", NONEXISTENT_BIN_TS)]);
    let profile_key = "a".repeat(64);

    #[cfg(windows)]
    let endpoint = format!(r"\\.\pipe\t3-managed-token-test-{}", uuid::Uuid::new_v4());
    #[cfg(unix)]
    let endpoint = temp
        .path()
        .join("managed.sock")
        .to_string_lossy()
        .into_owned();

    let srv_handle = spawn_mock_managed_token_server(endpoint.clone());
    tokio::task::yield_now().await;

    let home_path = temp.path().to_path_buf();
    let endpoint_arg = endpoint.clone();
    let profile_arg = profile_key.clone();
    let out = tokio::task::spawn_blocking(move || {
        run_t3(
            &[
                "__managed-chatgpt-token",
                &endpoint_arg,
                "inst-1",
                "env-1",
                &profile_arg,
            ],
            &isolated_env(&home_path, &[("T3_BIN_TS", NONEXISTENT_BIN_TS)]),
        )
    })
    .await??;

    srv_handle
        .await?
        .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8(out.stdout)?, "bare-jwt-token");
    assert_eq!(out.stderr, [] as [u8; 0]);

    // Unreachable endpoint emits exact reconnect error and exits 1.
    let miss_out = run_t3(
        &[
            "__managed-chatgpt-token",
            r"\\.\pipe\t3-nonexistent-pipe-test",
            "inst-1",
            "env-1",
            &profile_key,
        ],
        &env,
    )?;
    assert_eq!(miss_out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8(miss_out.stderr)?,
        "Could not access the managed ChatGPT connection. Reconnect in T3 Code.\n"
    );
    Ok(())
}
