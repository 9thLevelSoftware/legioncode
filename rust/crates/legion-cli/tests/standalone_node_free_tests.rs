//! End-to-end Node-free standalone verification (`LEGIONCODE_BIN_TS=Z:\nonexistent\bin.ts`) across
//! `legioncode serve` (`static_dir`, outbox `Replay (dev)` + git checkpoint, `terminal.*` RPC), `legioncode project`,
//! `legioncode pair`, `POST /api/auth/pair`, `legioncode __ssh-helper`, `legioncode trace summary`, and `legioncode thread`.

#[path = "support/e2e_helpers.rs"]
mod e2e_helpers;
mod support;
#[path = "support/ws_client.rs"]
mod ws_client;

use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use legion_cli::serve::{RunningServer, ServeConfig, ServeMode, spawn_server};
use legion_contracts::ids::{CommandId, MessageId, ProjectId, ProviderInstanceId, ThreadId};
use legion_contracts::orchestration::{
    CreationSource, InitialThreadMessage, ModelSelection, OrchestrationActor, OrchestrationCommand,
    ProviderInteractionMode, ThreadCreateCommand,
};
use legion_contracts::policy::RuntimeMode;
use legion_protocol::rpc_codec::RpcExit;
use legion_scm::checkpoints::store::CheckpointStore;
use reqwest::header::AUTHORIZATION;
use serde_json::{Value, json};

use e2e_helpers::{init_git_repo, rpc_call};
use support::{TestResult, isolated_env, run_legioncode};
use ws_client::WsTestClient;

const NONEXISTENT_BIN_TS: &str = r"Z:\nonexistent\bin.ts";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn standalone_node_free_end_to_end_showcase() -> TestResult {
    let temp = tempfile::tempdir()?;
    let base_dir = temp.path().join("e2e-home");
    let static_dir = temp.path().join("dist");
    let repo_dir = temp.path().join("repo");
    fs::create_dir_all(static_dir.join("assets"))?;
    fs::create_dir_all(&repo_dir)?;
    fs::write(
        static_dir.join("index.html"),
        "<html><body>LegionCode E2E</body></html>",
    )?;
    fs::write(
        static_dir.join("assets/main.js"),
        "console.log('legioncode');",
    )?;
    init_git_repo(&repo_dir)?;

    let server = spawn_server(ServeConfig {
        base_dir: base_dir.clone(),
        base_dir_is_explicit: true,
        port: 0,
        host: Some("127.0.0.1".to_owned()),
        mode: ServeMode::Web,
        no_browser: true,
        lan: false,
        static_dir: Some(static_dir),
        cwd: repo_dir.clone(),
    })
    .await?;

    let base_str = base_dir.to_string_lossy().into_owned();
    let cli_env = isolated_env(temp.path(), &[("LEGIONCODE_BIN_TS", NONEXISTENT_BIN_TS)]);
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;

    verify_static_assets(&http, server.origin()).await?;
    let project_id =
        verify_live_project_and_pair(&http, &server, &repo_dir, &base_str, &cli_env).await?;
    verify_ssh_helper_and_traces(&http, &server, &base_str, &cli_env).await?;
    verify_replay_turn_checkpoint_terminal_and_thread(&server, &project_id, &base_str, &cli_env)
        .await?;

    server.shutdown().await?;
    Ok(())
}

async fn verify_static_assets(http: &reqwest::Client, origin: &str) -> TestResult {
    let idx = http.get(format!("{origin}/")).send().await?;
    assert_eq!(idx.status(), 200);
    assert!(idx.text().await?.contains("LegionCode E2E"));

    let asset = http.get(format!("{origin}/assets/main.js")).send().await?;
    assert_eq!(asset.status(), 200);
    assert!(asset.text().await?.contains("legioncode"));

    let spa = http.get(format!("{origin}/threads/123")).send().await?;
    assert_eq!(spa.status(), 200);
    assert!(spa.text().await?.contains("LegionCode E2E"));
    Ok(())
}

async fn verify_live_project_and_pair(
    http: &reqwest::Client,
    server: &RunningServer,
    repo_dir: &Path,
    base_str: &str,
    cli_env: &[(&str, &str)],
) -> Result<String, Box<dyn std::error::Error>> {
    let repo_str = repo_dir.to_string_lossy().into_owned();
    let add_out = run_legioncode(
        &[
            "project",
            "add",
            &repo_str,
            "--title",
            "E2E Repo",
            "--base-dir",
            base_str,
        ],
        cli_env,
    )?;
    assert!(
        add_out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&add_out.stderr)
    );
    let add_stdout = String::from_utf8(add_out.stdout)?;
    assert!(add_stdout.starts_with("Added project "));

    let pair_out = run_legioncode(
        &["pair", "--base-dir", base_str, "--label", "Phone"],
        cli_env,
    )?;
    assert!(
        pair_out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&pair_out.stderr)
    );
    let pair_stdout = String::from_utf8(pair_out.stdout)?;
    assert!(pair_stdout.contains("Pairing with "));
    assert!(pair_stdout.contains('█'));
    assert!(pair_stdout.contains(&format!(
            "Pairing URL: {}:",
            server
                .origin()
                .split(':')
                .take(2)
                .collect::<Vec<_>>()
                .join(":")
        )));
    let token = pair_stdout
        .lines()
        .find_map(|line| line.strip_prefix("Token: "))
        .ok_or("missing Token line in `legioncode pair` output")?;

    let exchanged = http
        .post(format!("{}/api/auth/pair", server.origin()))
        .json(&json!({ "token": token, "clientLabel": "e2e-browser" }))
        .send()
        .await?;
    assert_eq!(exchanged.status(), 200);
    let bearer = exchanged.json::<Value>().await?["accessToken"]
        .as_str()
        .ok_or("missing accessToken")?
        .to_owned();

    let projects_resp = http
        .get(format!("{}/api/projects", server.origin()))
        .header(AUTHORIZATION, format!("Bearer {bearer}"))
        .send()
        .await?;
    assert_eq!(projects_resp.status(), 200);
    let projects_json: Value = projects_resp.json().await?;
    let project_id = projects_json["projects"][0]["id"]
        .as_str()
        .ok_or("missing persisted project id")?
        .to_owned();
    assert_eq!(projects_json["projects"][0]["title"], "E2E Repo");
    Ok(project_id)
}

async fn verify_ssh_helper_and_traces(
    http: &reqwest::Client,
    server: &RunningServer,
    base_str: &str,
    cli_env: &[(&str, &str)],
) -> TestResult {
    let rt_path = server
        .paths()
        .server_runtime_state_path
        .to_string_lossy()
        .into_owned();
    let rt_out = run_legioncode(&["__ssh-helper", "runtime-port", &rt_path], cli_env)?;
    assert!(rt_out.status.success());
    let rt_text = String::from_utf8(rt_out.stdout)?;
    assert_eq!(rt_text, format!("{} {}", std::process::id(), server.port()));
    assert!(!rt_text.ends_with('\n'));

    let keep_out = run_legioncode(
        &[
            "__ssh-helper",
            "keep-server",
            &rt_path,
            &std::process::id().to_string(),
        ],
        cli_env,
    )?;
    assert!(keep_out.status.success());

    let owned_out = run_legioncode(&["__ssh-helper", "home-owned", base_str], cli_env)?;
    assert!(owned_out.status.success());

    let now_ns = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let otlp = json!({
        "resourceSpans": [{
            "resource": { "attributes": [{ "key": "service.name", "value": { "stringValue": "legioncode-e2e" } }] },
            "scopeSpans": [{ "spans": [{ "name": "e2e.otlp.span", "startTimeUnixNano": now_ns.to_string(), "endTimeUnixNano": now_ns.saturating_add(5_000_000).to_string() }] }]
        }]
    });
    let trace_post = http
        .post(format!("{}/api/observability/v1/traces", server.origin()))
        .json(&otlp)
        .send()
        .await?;
    assert_eq!(trace_post.status(), 200);

    let traces_file = server.paths().logs_dir.join("traces.ndjson");
    let traces_str = traces_file.to_string_lossy().into_owned();
    let mut trace_env = cli_env.to_vec();
    trace_env.push(("LEGIONCODE_TRACE_FILE", &traces_str));
    let trace_out = run_legioncode(&["trace", "summary"], &trace_env)?;
    assert!(trace_out.status.success());
    assert!(String::from_utf8(trace_out.stdout)?.contains("e2e.otlp.span"));
    Ok(())
}

async fn verify_replay_turn_checkpoint_terminal_and_thread(
    server: &RunningServer,
    project_id: &str,
    base_str: &str,
    cli_env: &[(&str, &str)],
) -> TestResult {
    let mut ws = WsTestClient::connect(server.addr(), "/ws?orchestrationProtocol=2").await?;
    let thread_id = ThreadId::new("thread-e2e-replay")?;
    let create_cmd = OrchestrationCommand::ThreadCreate(ThreadCreateCommand {
        created_by: OrchestrationActor::User,
        creation_source: CreationSource::Web,
        command_id: CommandId::new("cmd-e2e-create-1")?,
        thread_id: thread_id.clone(),
        project_id: ProjectId::new(project_id)?,
        title: "Replay E2E".to_owned(),
        generate_title: None,
        model_selection: ModelSelection {
            instance_id: ProviderInstanceId::new("replay-dev")?,
            model: "replay".to_owned(),
            options: None,
        },
        runtime_mode: RuntimeMode::Auto,
        interaction_mode: ProviderInteractionMode::Default,
        branch: None,
        worktree_path: None,
        link_origin: None,
        delegated_from: None,
        imported_native_thread: None,
        initial_message: Some(InitialThreadMessage {
            message_id: MessageId::new("msg-e2e-1")?,
            text: "Run standalone replay turn".to_owned(),
            attachments: Vec::new(),
            context: None,
            title_seed: None,
            scheduled_task_id: None,
            sender_thread_id: None,
            output_schema: None,
            workspace_strategy: None,
        }),
    });
    let exit = rpc_call(
        &mut ws,
        "req-create",
        "orchestration.v2.dispatchCommand",
        serde_json::to_value(&create_cmd)?,
    )
    .await?;
    assert!(matches!(exit, RpcExit::Success { .. }));

    server
        .outbox_worker()
        .ok_or("missing outbox worker")?
        .drain()
        .await?;
    {
        let conn = server
            .orchestrator()
            .sink()
            .db()
            .lock()
            .map_err(|_| "db lock")?;
        let blob = CheckpointStore::new(&conn)
            .get_diff_blob(&thread_id, 0, 1)?
            .ok_or("missing git checkpoint diff blob after Replay turn")?;
        assert!(blob.diff.contains("dirty-content"));
    }

    let list_out = run_legioncode(
        &["thread", "list", "--base-dir", base_str, "--all", "--json"],
        cli_env,
    )?;
    assert!(
        list_out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&list_out.stderr)
    );
    let listed: Value = serde_json::from_slice(&list_out.stdout)?;
    assert_eq!(listed[0]["threadId"], "thread-e2e-replay");

    let read_out = run_legioncode(
        &[
            "thread",
            "read",
            "thread-e2e-replay",
            "--base-dir",
            base_str,
        ],
        cli_env,
    )?;
    assert!(read_out.status.success());
    assert!(String::from_utf8(read_out.stdout)?.contains("Run standalone replay turn"));
    verify_live_terminal_ws(&mut ws).await
}

async fn verify_live_terminal_ws(ws: &mut WsTestClient) -> TestResult {
    let _ = rpc_call(
        ws,
        "t-open",
        "terminal.open",
        json!({ "threadId": "thread-e2e-replay", "terminalId": "term-1", "cols": 80, "rows": 24 }),
    )
    .await?;
    let _ = rpc_call(
        ws,
        "t-write",
        "terminal.write",
        json!({ "threadId": "thread-e2e-replay", "terminalId": "term-1", "data": "echo hi" }),
    )
    .await?;
    let _ = rpc_call(
        ws,
        "t-resize",
        "terminal.resize",
        json!({ "threadId": "thread-e2e-replay", "terminalId": "term-1", "cols": 100, "rows": 30 }),
    )
    .await?;
    let RpcExit::Success { value: snap } = rpc_call(
        ws,
        "t-snap",
        "terminal.snapshot",
        json!({ "threadId": "thread-e2e-replay", "terminalId": "term-1" }),
    )
    .await?
    else {
        return Err("expected terminal.snapshot Success".into());
    };
    assert_eq!(snap["cols"], 100);
    assert!(
        snap["viewportText"]
            .as_str()
            .unwrap_or("")
            .contains("echo hi")
    );
    let _ = rpc_call(
        ws,
        "t-close",
        "terminal.close",
        json!({ "threadId": "thread-e2e-replay", "terminalId": "term-1" }),
    )
    .await?;
    Ok(())
}
