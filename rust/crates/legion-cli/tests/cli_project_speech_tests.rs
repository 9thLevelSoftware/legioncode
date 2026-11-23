//! Integration tests for offline `legioncode project add|rename|remove`, verbatim normalization,
//! live-server delegation, and synchronous `legioncode __speech-worker`.

mod support;

use std::fs;
use std::io::Write as _;
use std::process::{Command, Stdio};

use legion_cli::cmd_project::strip_windows_verbatim_prefix;
use legion_cli::serve::{ServeConfig, ServeMode, spawn_server};
use legion_persistence::project_store::{archive_project, list_non_deleted_projects};
use legion_speech::types::TranscriptionOptions;
use legion_speech::vad::VAD_FRAME_SAMPLES;
use legion_speech::worker_protocol::{
    WorkerAudioPayload, WorkerCommand, WorkerEvent, decode_worker_event, encode_worker_command,
};

use support::{TestResult, isolated_env, run_legioncode};

#[test]
fn offline_project_add_rename_unarchive_and_remove_work_synchronously() -> TestResult {
    let temp = tempfile::tempdir()?;
    let base_dir = temp.path().join("proj-home");
    let workspace = temp.path().join("workspace-alpha");
    fs::create_dir_all(&workspace)?;
    let env = isolated_env(temp.path(), &[]);

    let base_str = base_dir.to_string_lossy().into_owned();
    let ws_raw = format!(
        "{}{}",
        workspace.to_string_lossy(),
        std::path::MAIN_SEPARATOR
    );
    let out_add = run_legioncode(&["project", "add", &ws_raw, "--base-dir", &base_str], &env)?;
    assert!(out_add.status.success());
    let add_text = String::from_utf8(out_add.stdout)?;
    assert!(add_text.starts_with("Added project "));
    assert!(add_text.contains("(workspace-alpha)"));
    assert!(!add_text.contains(r"\\?\"));

    let db_path = base_dir.join("userdata").join("statev2.sqlite");
    let rows = list_non_deleted_projects(&db_path)?;
    assert_eq!(rows.len(), 1);
    let project_id = rows[0].id.clone();

    // Duplicate add fails cleanly.
    let out_dup = run_legioncode(&["project", "add", &ws_raw, "--base-dir", &base_str], &env)?;
    assert!(!out_dup.status.success());
    assert!(String::from_utf8_lossy(&out_dup.stderr).contains("ProjectAlreadyExistsError"));

    // Rename, idempotent rename, archive + unarchive, and remove.
    let out_rename = run_legioncode(
        &[
            "project",
            "rename",
            &ws_raw,
            "Alpha Renamed",
            "--base-dir",
            &base_str,
        ],
        &env,
    )?;
    assert!(out_rename.status.success());
    assert_eq!(
        String::from_utf8(out_rename.stdout)?.trim(),
        format!("Renamed project {project_id} to Alpha Renamed.")
    );

    let out_same = run_legioncode(
        &[
            "project",
            "rename",
            &project_id,
            "Alpha Renamed",
            "--base-dir",
            &base_str,
        ],
        &env,
    )?;
    assert!(out_same.status.success());
    assert_eq!(
        String::from_utf8(out_same.stdout)?.trim(),
        format!("Project {project_id} is already named Alpha Renamed.")
    );

    archive_project(&db_path, &project_id)?;
    let out_unarchive = run_legioncode(
        &[
            "project",
            "add",
            &ws_raw,
            "--title",
            "Alpha Restored",
            "--base-dir",
            &base_str,
        ],
        &env,
    )?;
    assert!(out_unarchive.status.success());
    assert!(
        String::from_utf8(out_unarchive.stdout)?.starts_with(&format!(
            "Unarchived project {project_id} (Alpha Restored) at "
        ))
    );

    let out_remove = run_legioncode(
        &["project", "remove", &ws_raw, "--base-dir", &base_str],
        &env,
    )?;
    assert!(out_remove.status.success());
    assert_eq!(
        String::from_utf8(out_remove.stdout)?.trim(),
        format!("Removed project {project_id} (Alpha Restored).")
    );
    assert_eq!(list_non_deleted_projects(&db_path)?.len(), 0);

    // Removing an already-deleted project fails with ProjectNotFoundError.
    let out_missing = run_legioncode(
        &["project", "remove", &project_id, "--base-dir", &base_str],
        &env,
    )?;
    assert!(!out_missing.status.success());
    assert!(String::from_utf8_lossy(&out_missing.stderr).contains("ProjectNotFoundError"));
    Ok(())
}

#[test]
fn windows_verbatim_prefix_strip_and_missing_dir_error() -> TestResult {
    #[cfg(windows)]
    {
        assert_eq!(
            strip_windows_verbatim_prefix(r"\\?\C:\work\proj"),
            r"C:\work\proj"
        );
        assert_eq!(
            strip_windows_verbatim_prefix(r"\\?\UNC\server\share\proj"),
            r"\\server\share\proj"
        );
    }
    #[cfg(not(windows))]
    {
        assert_eq!(strip_windows_verbatim_prefix("/work/proj"), "/work/proj");
    }

    let temp = tempfile::tempdir()?;
    let env = isolated_env(temp.path(), &[]);
    let missing_ws = temp.path().join("missing-dir");
    let out = run_legioncode(
        &[
            "project",
            "add",
            &missing_ws.to_string_lossy(),
            "--base-dir",
            &temp.path().join("home").to_string_lossy(),
        ],
        &env,
    )?;
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("WorkspaceRootNotExistsError"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_runs_natively_over_http_against_live_server_and_honors_kill_switch() -> TestResult
{
    let temp = tempfile::tempdir()?;
    let base_dir = temp.path().join("live-proj-home");
    let workspace = temp.path().join("ws-live");
    fs::create_dir_all(&workspace)?;

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

    let log_file = temp.path().join("project-fallback.jsonl");
    let log_str = log_file.to_string_lossy().into_owned();
    let env = isolated_env(
        temp.path(),
        &[
            ("RUST_ANYTHING_FALLBACK_LOG", &log_str),
            ("LEGIONCODE_BIN_TS", r"Z:\nonexistent\bin.ts"),
        ],
    );
    let out = run_legioncode(
        &[
            "project",
            "add",
            &workspace.to_string_lossy(),
            "--title",
            "Live Added",
            "--base-dir",
            &base_dir.to_string_lossy(),
        ],
        &env,
    )?;
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout)?;
    assert!(stdout.starts_with("Added project "));
    assert!(stdout.contains("(Live Added)"));
    assert!(!log_file.exists());

    // `--remote-url` executes natively without delegating to Node.
    let out_remote = run_legioncode(
        &[
            "project",
            "add",
            &workspace.to_string_lossy(),
            "--remote-url",
            "https://remote.example",
            "--base-dir",
            &base_dir.to_string_lossy(),
        ],
        &env,
    )?;
    assert!(!out_remote.status.success());
    assert!(!log_file.exists());

    // Kill switch delegation even when offline.
    server.shutdown().await?;
    let log_offline = temp.path().join("project-killswitch.jsonl");
    let log_offline_str = log_offline.to_string_lossy().into_owned();
    let fake_project = temp.path().join("fake-project.cjs");
    fs::write(
        &fake_project,
        "process.stdout.write('Added project stub\\n');\n",
    )?;
    let fake_project_str = fake_project.to_string_lossy().into_owned();
    let env_ks = isolated_env(
        temp.path(),
        &[
            ("LEGIONCODE_BIN_TS", &fake_project_str),
            ("RUST_ANYTHING_UNIT_PROJECT", "original"),
            ("RUST_ANYTHING_FALLBACK_LOG", &log_offline_str),
        ],
    );
    let out_ks = run_legioncode(
        &[
            "project",
            "add",
            &workspace.to_string_lossy(),
            "--base-dir",
            &temp.path().join("ks-home").to_string_lossy(),
        ],
        &env_ks,
    )?;
    assert!(out_ks.status.success());
    assert!(fs::read_to_string(&log_offline)?.contains(r#""unit":"PROJECT""#));
    Ok(())
}

#[test]
fn speech_worker_executes_ndjson_stdio_and_honors_kill_switch() -> TestResult {
    let temp = tempfile::tempdir()?;
    let mut child = Command::new(env!("CARGO_BIN_EXE_legioncode"))
        .arg("__speech-worker")
        .env_remove("RUST_ANYTHING_UNIT_SPEECH_WORKER")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    {
        let mut stdin = child.stdin.take().ok_or("missing child stdin")?;
        let pcm16: Vec<i16> = (0..VAD_FRAME_SAMPLES * 2)
            .map(|i| if (i / 16) % 2 == 0 { 8000 } else { -8000 })
            .collect();
        stdin.write_all(
            encode_worker_command(&WorkerCommand::Config {
                options: TranscriptionOptions::default(),
                streaming: true,
                model_path: None,
                vad_path: None,
            })?
            .as_bytes(),
        )?;
        stdin.write_all(
            encode_worker_command(&WorkerCommand::AudioChunk {
                audio: WorkerAudioPayload::from_pcm16(pcm16.clone()),
                final_chunk: true,
            })?
            .as_bytes(),
        )?;
        stdin.write_all(
            encode_worker_command(&WorkerCommand::Transcribe {
                audio: WorkerAudioPayload::from_pcm16(pcm16),
                options: None,
            })?
            .as_bytes(),
        )?;
        stdin.write_all(encode_worker_command(&WorkerCommand::Reset)?.as_bytes())?;
        stdin.write_all(encode_worker_command(&WorkerCommand::Shutdown)?.as_bytes())?;
        stdin.flush()?;
    }

    let output = child.wait_with_output()?;
    assert!(
        output.status.success(),
        "__speech-worker stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout)?;
    let events: Vec<WorkerEvent> = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(decode_worker_event)
        .collect::<Result<Vec<_>, _>>()?;

    assert!(matches!(events.first(), Some(WorkerEvent::Ready { .. })));
    assert!(
        events
            .iter()
            .any(|ev| matches!(ev, WorkerEvent::Vad { .. }))
    );
    assert_eq!(
        events
            .iter()
            .filter(|ev| matches!(ev, WorkerEvent::Transcript { is_final: true, .. }))
            .count(),
        2
    );
    assert!(matches!(
        events.last(),
        Some(WorkerEvent::Stopped { reason, .. }) if reason == "shutdown"
    ));

    // Kill switch logs `SPEECH_WORKER`.
    let log_file = temp.path().join("speech-fallback.jsonl");
    let log_str = log_file.to_string_lossy().into_owned();
    let env_ks = isolated_env(
        temp.path(),
        &[
            ("RUST_ANYTHING_UNIT_SPEECH_WORKER", "original"),
            ("RUST_ANYTHING_FALLBACK_LOG", &log_str),
        ],
    );
    let out_ks = run_legioncode(&["__speech-worker", "--help"], &env_ks)?;
    let _ = out_ks;
    assert!(fs::read_to_string(&log_file)?.contains(r#""unit":"SPEECH_WORKER""#));
    Ok(())
}
