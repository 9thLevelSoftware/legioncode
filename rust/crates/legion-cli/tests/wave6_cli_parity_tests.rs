//! Focused Wave 6 CLI tests for `workflow`, `feedback`, help/completions, and `thread` parity.

mod support;

use std::fs;

use legion_cli::serve::{ServeConfig, ServeMode, spawn_server};
use legion_workflows::feedback::FeedbackStore;
use legion_workflows::feedback_types::{FeedbackMode, FeedbackSource, SubmitFeedbackInput};
use legion_workflows::trace_types::FeedbackKind;
use serde_json::{Value, json};

use support::{TestResult, isolated_env, run_legioncode};

#[test]
fn help_and_shell_completions_include_workflow_and_feedback() -> TestResult {
    let temp = tempfile::tempdir()?;
    let env = isolated_env(temp.path(), &[]);

    let help_wf = run_legioncode(&["help", "workflow"], &env)?;
    assert!(help_wf.status.success());
    let help_wf_out = String::from_utf8_lossy(&help_wf.stdout);
    assert!(help_wf_out.contains("Durable workflow runs") || help_wf_out.contains("workflow"));

    let help_fb = run_legioncode(&["help", "feedback"], &env)?;
    assert!(help_fb.status.success());
    let help_fb_out = String::from_utf8_lossy(&help_fb.stdout);
    assert!(help_fb_out.contains("local feedback"));

    for shell in ["bash", "zsh", "fish", "powershell"] {
        let comp = run_legioncode(&["--completions", shell], &env)?;
        assert!(comp.status.success(), "shell {shell} failed");
        let comp_out = String::from_utf8_lossy(&comp.stdout);
        assert!(comp_out.contains("workflow"), "{shell} missing workflow");
        assert!(comp_out.contains("feedback"), "{shell} missing feedback");
    }
    Ok(())
}

#[test]
fn feedback_list_respects_off_setting_and_reads_local_reports() -> TestResult {
    let temp = tempfile::tempdir()?;
    let env = isolated_env(temp.path(), &[]);
    let base_arg = temp.path().to_string_lossy().into_owned();

    // Missing settings defaults to off -> fails cleanly without creating DB.
    let off_out = run_legioncode(&["feedback", "list", "--base-dir", &base_arg], &env)?;
    assert!(!off_out.status.success());
    assert!(String::from_utf8_lossy(&off_out.stderr).contains("Agent feedback is off"));
    assert!(!temp.path().join("userdata/statev2.sqlite").exists());

    // Turn on-request + seed one report locally (`sent = 0`) and read with `--json`.
    let userdata = temp.path().join("userdata");
    fs::create_dir_all(&userdata)?;
    fs::write(
        userdata.join("settings.json"),
        serde_json::to_string_pretty(&json!({ "agentFeedback": { "mode": "on-request" } }))?,
    )?;
    let conn = std::sync::Arc::new(std::sync::Mutex::new(rusqlite::Connection::open(
        userdata.join("statev2.sqlite"),
    )?));
    let store = FeedbackStore::new(conn)?;
    let _ = store.submit(
        FeedbackMode::Auto,
        &SubmitFeedbackInput {
            kind: FeedbackKind::Bug,
            subject: "legioncode thread send",
            expected: "works",
            actual: "failed",
            workaround: "",
            user_requested: false,
            turns: 1,
            source: FeedbackSource::Cli,
            thread_id: None,
            run_id: None,
            sensitive: &[],
            registered_tools: &[],
            cli_commands: &["legioncode thread send"],
            trace_runs: &[],
            trace_items: &[],
        },
    )?;

    let list_out = run_legioncode(
        &["feedback", "list", "--base-dir", &base_arg, "--json"],
        &env,
    )?;
    assert!(
        list_out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&list_out.stderr)
    );
    let parsed: Vec<Value> = serde_json::from_slice(&list_out.stdout)?;
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0]["subject"], "legioncode thread send");
    assert_eq!(parsed[0]["sent"], false);
    Ok(())
}

async fn run_cli_async(
    base_dir: &std::path::Path,
    args: &[&str],
) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    let base_path = base_dir.to_path_buf();
    let owned: Vec<String> = args.iter().map(|s| (*s).to_owned()).collect();
    let out = tokio::task::spawn_blocking(move || {
        let env = isolated_env(&base_path, &[]);
        let refs: Vec<&str> = owned.iter().map(String::as_str).collect();
        run_legioncode(&refs, &env)
    })
    .await??;
    Ok(out)
}

#[tokio::test]
async fn workflow_cli_store_set_list_and_delete_against_live_server() -> TestResult {
    let temp = tempfile::tempdir()?;
    let userdata = temp.path().join("userdata");
    let wf_dir = userdata.join("workflows");
    fs::create_dir_all(&wf_dir)?;
    fs::write(
        userdata.join("settings.json"),
        serde_json::to_string_pretty(&json!({ "workflowsEnabled": true }))?,
    )?;
    fs::write(
        wf_dir.join("sample.ts"),
        "Legion.workflow({ name: \"sample\", version: 1, triggers: [], run: () => Effect.gen(function* () { return { ok: true }; }) });",
    )?;
    let server = spawn_server(ServeConfig {
        base_dir: temp.path().to_path_buf(),
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

    let base_arg = temp.path().to_string_lossy().into_owned();
    let remote = server.origin().to_owned();

    // When `require_auth == false` (test server), direct `workflow store` hits `/mcp`.
    let set_res = run_cli_async(
        temp.path(),
        &[
            "workflow",
            "store",
            "set",
            "alpha",
            "{\"ok\":true}",
            "--base-dir",
            &base_arg,
        ],
    )
    .await?;
    assert!(
        set_res.status.success(),
        "stdout: {} / stderr: {}",
        String::from_utf8_lossy(&set_res.stdout),
        String::from_utf8_lossy(&set_res.stderr)
    );

    let list_res = run_cli_async(
        temp.path(),
        &[
            "workflow",
            "store",
            "list",
            "--base-dir",
            &base_arg,
            "--json",
        ],
    )
    .await?;
    assert!(
        list_res.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&list_res.stderr)
    );
    let parsed: Value = serde_json::from_slice(&list_res.stdout)?;
    assert_eq!(parsed["entries"][0]["key"], "alpha");
    assert_eq!(parsed["entries"][0]["value"]["ok"], true);

    let del_res = run_cli_async(
        temp.path(),
        &[
            "workflow",
            "store",
            "delete",
            "alpha",
            "--base-dir",
            &base_arg,
        ],
    )
    .await?;
    assert!(
        del_res.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&del_res.stderr)
    );

    let _ = remote;
    server.shutdown().await?;
    Ok(())
}
