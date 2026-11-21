//! Zero-Node integration tests for remote `--remote-url`/`--token` (`project` and `thread`),
//! T3 Connect config/status, `pair --json`, and `AcousticAsrEngine` `__speech-worker`.

use std::fs;
use std::io::Write as _;
use std::process::{Command, Stdio};

use serde_json::{Value, json};
use t3_cli::serve::{ServeConfig, ServeMode, spawn_server};
use t3_persistence::pairing_tokens::{IssuePairingTokenInput, issue_pairing_token};
use t3_speech::acoustic_model::AcousticModelWeights;
use t3_speech::vad::VAD_FRAME_SAMPLES;
use t3_speech::worker_protocol::{
    WorkerAudioPayload, WorkerCommand, WorkerEvent, decode_worker_event, encode_worker_command,
};

use crate::support::{TestResult, isolated_env, run_t3};

const NONEXISTENT_BIN_TS: &str = r"Z:\nonexistent\bin.ts";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_project_thread_pair_and_cloud_status_run_without_node() -> TestResult {
    let temp = tempfile::tempdir()?;
    let srv_home = temp.path().join("remote-srv-home");
    let srv_home_str = srv_home.to_string_lossy().into_owned();
    let ws = temp.path().join("remote-ws");
    fs::create_dir_all(&ws)?;
    let ws_str = ws.to_string_lossy().into_owned();

    let server = spawn_server(ServeConfig {
        base_dir: srv_home.clone(),
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

    let now_ms = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis(),
    )?;
    let issued = issue_pairing_token(
        &server.paths().db_path,
        &IssuePairingTokenInput::default().with_ttl_ms(60_000),
        now_ms,
    )?;
    let pair_json: Value = reqwest::Client::new()
        .post(format!("{}/api/auth/pair", server.origin()))
        .json(&json!({ "token": issued.credential, "clientLabel": "stage8-remote" }))
        .send()
        .await?
        .json()
        .await?;
    let bearer = pair_json["accessToken"]
        .as_str()
        .ok_or("missing accessToken")?
        .to_owned();

    let empty_bin = temp.path().join("empty-bin");
    fs::create_dir_all(&empty_bin)?;
    let empty_str = empty_bin.to_string_lossy().into_owned();
    let log_file = temp.path().join("remote-fallback.jsonl");
    let log_str = log_file.to_string_lossy().into_owned();
    let env = isolated_env(
        temp.path(),
        &[
            ("PATH", &empty_str),
            ("T3_BIN_TS", NONEXISTENT_BIN_TS),
            ("RUST_ANYTHING_FALLBACK_LOG", &log_str),
        ],
    );

    verify_remote_project_and_thread(server.origin(), &ws_str, &bearer, &env)?;
    verify_pair_and_cloud_status(temp.path(), &srv_home_str, &empty_str, &log_str, &env)?;

    assert!(!log_file.exists(), "fallback log must stay absent");
    server.shutdown().await?;
    Ok(())
}

fn verify_remote_project_and_thread(
    origin: &str,
    ws_str: &str,
    bearer: &str,
    env: &[(&str, &str)],
) -> TestResult {
    let no_tok = run_t3(&["project", "add", ws_str, "--remote-url", origin], env)?;
    assert_eq!(no_tok.status.code(), Some(1));
    assert!(String::from_utf8(no_tok.stderr)?.contains("--remote-url requires --token"));

    let add_out = run_t3(
        &[
            "project",
            "add",
            ws_str,
            "--remote-url",
            origin,
            "--token",
            bearer,
            "--title",
            "Remote Proj",
        ],
        env,
    )?;
    assert!(
        add_out.status.success(),
        "add stderr: {}",
        String::from_utf8_lossy(&add_out.stderr)
    );
    assert!(String::from_utf8(add_out.stdout)?.contains("Added project"));

    let ren_out = run_t3(
        &[
            "project",
            "rename",
            ws_str,
            "Remote Renamed",
            "--remote-url",
            origin,
            "--token",
            bearer,
        ],
        env,
    )?;
    assert!(ren_out.status.success());
    assert!(String::from_utf8(ren_out.stdout)?.contains("Remote Renamed"));

    let del_out = run_t3(
        &[
            "project",
            "remove",
            ws_str,
            "--force",
            "--remote-url",
            origin,
            "--token",
            bearer,
        ],
        env,
    )?;
    assert!(del_out.status.success());

    let thr_list = run_t3(
        &[
            "thread",
            "list",
            "--all",
            "--remote-url",
            origin,
            "--token",
            bearer,
            "--json",
        ],
        env,
    )?;
    assert!(
        thr_list.status.success(),
        "thread list stderr: {}",
        String::from_utf8_lossy(&thr_list.stderr)
    );
    let thr_show = run_t3(
        &[
            "thread",
            "show",
            "nonexistent-thread",
            "--remote-url",
            origin,
            "--token",
            bearer,
        ],
        env,
    )?;
    assert_eq!(thr_show.status.code(), Some(1));
    Ok(())
}

fn verify_pair_and_cloud_status(
    temp_root: &std::path::Path,
    srv_home_str: &str,
    empty_str: &str,
    log_str: &str,
    env: &[(&str, &str)],
) -> TestResult {
    let pair_cmd = run_t3(&["pair", "--json", "--base-dir", srv_home_str], env)?;
    assert!(pair_cmd.status.success());
    let pair_payload: Value = serde_json::from_slice(&pair_cmd.stdout)?;
    assert!(pair_payload["pairingUrl"].as_str().is_some());
    assert!(pair_payload["credential"].as_str().is_some());

    let cloud_env = isolated_env(
        temp_root,
        &[
            ("PATH", empty_str),
            ("T3_BIN_TS", NONEXISTENT_BIN_TS),
            ("RUST_ANYTHING_FALLBACK_LOG", log_str),
            ("T3CODE_RELAY_URL", "https://relay.example.invalid"),
            ("T3CODE_CLERK_PUBLISHABLE_KEY", "pk_test_stage8"),
            ("T3CODE_CLERK_CLI_OAUTH_CLIENT_ID", "cli_stage8"),
        ],
    );
    let conn_json = run_t3(
        &["connect", "status", "--json", "--base-dir", srv_home_str],
        &cloud_env,
    )?;
    assert!(conn_json.status.success());
    let conn_val: Value = serde_json::from_slice(&conn_json.stdout)?;
    assert_eq!(conn_val["desired"], false);
    assert_eq!(conn_val["linked"], false);

    let st_json = run_t3(
        &["status", "--json", "--base-dir", srv_home_str],
        &cloud_env,
    )?;
    assert!(st_json.status.success());
    let st_val: Value = serde_json::from_slice(&st_json.stdout)?;
    assert!(st_val["connect"].is_object());
    Ok(())
}

#[test]
fn speech_worker_discover_model_and_acoustic_asr_voiced_pcm_zero_node() -> TestResult {
    let temp = tempfile::tempdir()?;
    let empty_bin = temp.path().join("empty-bin");
    fs::create_dir_all(&empty_bin)?;
    let empty_str = empty_bin.to_string_lossy().into_owned();
    let log_file = temp.path().join("speech-stage8-fallback.jsonl");
    let log_str = log_file.to_string_lossy().into_owned();
    let env = isolated_env(
        temp.path(),
        &[
            ("PATH", &empty_str),
            ("T3_BIN_TS", NONEXISTENT_BIN_TS),
            ("RUST_ANYTHING_FALLBACK_LOG", &log_str),
        ],
    );

    // 1. `__speech-worker discover` returns Safe Rust CPU backend list.
    let disc = run_t3(&["__speech-worker", "discover"], &env)?;
    assert_eq!(disc.status.code(), Some(0));
    let devices: Value = serde_json::from_slice(&disc.stdout)?;
    assert!(devices.as_array().is_some_and(|arr| {
        arr.iter()
            .any(|d| d["kind"] == "cpu" && d["available"] == true)
    }));

    // 2. `__speech-worker model <path>` inspects valid `.t3asr.json` prototype weights.
    let model_path = temp.path().join("custom.t3asr.json");
    let weights = AcousticModelWeights::default_english();
    fs::write(&model_path, serde_json::to_vec_pretty(&weights)?)?;
    let mod_out = run_t3(
        &["__speech-worker", "model", &model_path.to_string_lossy()],
        &env,
    )?;
    assert_eq!(
        mod_out.status.code(),
        Some(0),
        "model stderr: {}",
        String::from_utf8_lossy(&mod_out.stderr)
    );
    let mod_json: Value = serde_json::from_slice(&mod_out.stdout)?;
    assert_eq!(mod_json["format"], "t3_asr_json");
    assert!(mod_json["prototypeCount"].as_u64().is_some_and(|n| n > 0));

    // 3. `__speech-worker --stdio` emits non-empty acoustic transcript text for voiced 16kHz PCM.
    let mut child = Command::new(env!("CARGO_BIN_EXE_t3"))
        .args(["__speech-worker", "--stdio"])
        .env("PATH", &empty_str)
        .env("T3_BIN_TS", NONEXISTENT_BIN_TS)
        .env("RUST_ANYTHING_FALLBACK_LOG", &log_str)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    {
        const SINE_16: [i16; 16] = [
            0, 5358, 9899, 12934, 14000, 12934, 9899, 5358, 0, -5358, -9899, -12934, -14000,
            -12934, -9899, -5358,
        ];
        let mut stdin = child.stdin.take().ok_or("missing stdin")?;
        let pcm16: Vec<i16> = (0..VAD_FRAME_SAMPLES * 8)
            .map(|i| SINE_16[(i * 2 / 3) % SINE_16.len()])
            .collect();
        stdin.write_all(
            encode_worker_command(&WorkerCommand::Transcribe {
                audio: WorkerAudioPayload::from_pcm16(pcm16),
                options: None,
            })?
            .as_bytes(),
        )?;
        stdin.write_all(encode_worker_command(&WorkerCommand::Shutdown)?.as_bytes())?;
        stdin.flush()?;
    }
    let out = child.wait_with_output()?;
    assert!(out.status.success());
    let events: Vec<WorkerEvent> = String::from_utf8(out.stdout)?
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(decode_worker_event)
        .collect::<Result<Vec<_>, _>>()?;
    let transcript = events.iter().find_map(|ev| match ev {
        WorkerEvent::Transcript {
            is_final: true,
            text,
            ..
        } => Some(text.clone()),
        _ => None,
    });
    assert!(
        transcript.is_some_and(|t| !t.trim().is_empty()),
        "expected non-empty AcousticAsrEngine transcript, got events={events:?}"
    );
    assert!(!log_file.exists(), "fallback log must stay absent");
    Ok(())
}
