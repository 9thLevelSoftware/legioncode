//! Focused Wave 6 WS RPC & runtime integration tests on `legioncode serve`.

#[path = "support/ws_client.rs"]
mod ws_client;

use std::process::Command;

use legion_cli::serve::{RunningServer, ServeConfig, ServeMode, spawn_server};
use legion_contracts::ids::{CommandId, MessageId, ProjectId, ProviderInstanceId, ThreadId};
use legion_contracts::orchestration::{
    CreationSource, InitialThreadMessage, ModelSelection, OrchestrationActor, OrchestrationCommand,
    ProviderInteractionMode, ThreadCreateCommand,
};
use legion_contracts::policy::RuntimeMode;
use legion_orchestrator::decider::decide_command;
use legion_orchestrator::sink::projection_apply::DeciderThreadState;
use legion_persistence::project_store::{
    CreateProjectInput, create_project, ensure_project_store_schema,
};
use legion_protocol::rpc_codec::{
    ClientRpcFrame, RpcExit, RpcId, ServerRpcFrame, decode_server_frames, encode_client_frame,
};
use serde_json::{Value, json};
use tempfile::TempDir;

use ws_client::WsTestClient;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

async fn spawn_isolated() -> TestResult<(TempDir, RunningServer)> {
    let temp = tempfile::tempdir()?;
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
    Ok((temp, server))
}

async fn call_ws(
    ws: &mut WsTestClient,
    id: &str,
    tag: &str,
    payload: Value,
) -> TestResult<RpcExit> {
    let frame = ClientRpcFrame::Request {
        id: RpcId::from(id),
        tag: tag.to_owned(),
        payload,
        headers: Vec::new(),
        is_notification: false,
        discard: false,
        trace_id: None,
        span_id: None,
        sampled: None,
    };
    ws.send_text(&encode_client_frame(&frame)?).await?;
    loop {
        let frames = decode_server_frames(&ws.recv_text().await?)?;
        for frame in frames {
            match frame {
                ServerRpcFrame::Exit {
                    request_id: resp_id,
                    exit,
                } if resp_id == RpcId::from(id) => {
                    return Ok(exit);
                }
                _ => {}
            }
        }
    }
}

async fn call_ok(ws: &mut WsTestClient, id: &str, tag: &str, payload: Value) -> TestResult<Value> {
    match call_ws(ws, id, tag, payload).await? {
        RpcExit::Success { value } => Ok(value),
        RpcExit::Failure { cause } => Err(format!("{tag} failed unexpectedly: {cause:?}").into()),
    }
}

#[tokio::test]
async fn server_probe_usage_mcp_signin_and_assets_disposition_work() -> TestResult {
    let (temp, server) = spawn_isolated().await?;
    let mut ws = WsTestClient::connect(server.addr(), "/ws?orchestrationProtocol=2").await?;

    let probed = call_ok(&mut ws, "1", "server.probe", json!({})).await?;
    assert!(
        probed["serverTime"]
            .as_str()
            .is_some_and(|s| s.contains('T'))
    );

    let usage = call_ok(
        &mut ws,
        "2",
        "server.getUsageSummary",
        json!({ "sinceDay": "2026-01-01", "untilDay": "2026-01-31", "timeZone": "UTC" }),
    )
    .await?;
    assert_eq!(usage["contractVersion"], 6);

    let invalid_usage = call_ws(
        &mut ws,
        "3",
        "server.getUsageSummary",
        json!({ "sinceDay": "2026-02-01", "untilDay": "2026-01-01", "timeZone": "UTC" }),
    )
    .await?;
    assert!(matches!(invalid_usage, RpcExit::Failure { .. }));

    let signin_unstarted = call_ws(
        &mut ws,
        "4",
        "provider.mcpSignIn.start",
        json!({ "threadId": "thr-1", "providerThreadId": "p-1", "server": "linear" }),
    )
    .await?;
    assert!(matches!(signin_unstarted, RpcExit::Failure { .. }));
    let signin_missing = call_ws(
        &mut ws,
        "5",
        "provider.mcpSignIn.complete",
        json!({ "threadId": "thr-1", "providerThreadId": "p-1", "server": "missing", "redirectUrl": "http://127.0.0.1/cb?code=1&state=x" }),
    )
    .await?;
    assert!(matches!(signin_missing, RpcExit::Failure { .. }));

    let doc = temp.path().join("notes.txt");
    std::fs::write(&doc, "hello world")?;
    let url_res = call_ok(
        &mut ws,
        "6",
        "assets.createUrl",
        json!({
            "resource": {
                "_tag": "draft-workspace-file",
                "cwd": temp.path(),
                "path": "notes.txt",
                "disposition": "attachment"
            }
        }),
    )
    .await?;
    let rel = url_res["relativeUrl"]
        .as_str()
        .ok_or("missing relativeUrl")?;
    let get_resp = reqwest::get(format!("{}{rel}", server.origin())).await?;
    assert_eq!(get_resp.status(), 200);
    assert_eq!(
        get_resp
            .headers()
            .get("content-disposition")
            .and_then(|v| v.to_str().ok()),
        Some("attachment; filename=\"notes.txt\"")
    );

    server.shutdown().await?;
    Ok(())
}

fn init_git_repo(repo: &std::path::Path) -> TestResult {
    std::fs::create_dir_all(repo)?;
    for args in [
        vec!["init", "-b", "main"],
        vec!["config", "user.email", "t@example.com"],
        vec!["config", "user.name", "Tester"],
    ] {
        assert!(
            Command::new("git")
                .args(args)
                .current_dir(repo)
                .status()?
                .success()
        );
    }
    std::fs::write(repo.join("README.md"), "hi\n")?;
    for args in [
        vec!["add", "."],
        vec!["commit", "-m", "init"],
        vec!["branch", "other"],
    ] {
        assert!(
            Command::new("git")
                .args(args)
                .current_dir(repo)
                .status()?
                .success()
        );
    }
    Ok(())
}

#[tokio::test]
async fn vcs_refs_digest_reveal_and_peer_links_work() -> TestResult {
    let (temp, server) = spawn_isolated().await?;
    let repo = temp.path().join("repo");
    init_git_repo(&repo)?;

    let mut ws = WsTestClient::connect(server.addr(), "/ws?orchestrationProtocol=2").await?;
    let status_res = call_ok(&mut ws, "v1", "vcs.refreshStatus", json!({ "cwd": repo })).await?;
    assert_eq!(status_res["isRepo"], true);
    let digest = status_res["refsDigest"]
        .as_str()
        .ok_or("missing refsDigest")?;
    assert_eq!(digest.len(), 32);

    let local_status = call_ok(
        &mut ws,
        "v2",
        "vcs.refreshStatus",
        json!({ "cwd": repo, "localOnly": true }),
    )
    .await?;
    assert!(local_status.get("refsDigest").is_none());

    let names = call_ok(&mut ws, "p1", "peerLinks.names", json!({})).await?;
    assert_eq!(names["links"], json!([]));
    let unlinked = call_ok(
        &mut ws,
        "p2",
        "peerLinks.unlink",
        json!({ "environmentId": "missing-env" }),
    )
    .await?;
    assert_eq!(unlinked, json!({ "removed": false, "endedThere": false }));

    let reveal_missing = call_ws(
        &mut ws,
        "f1",
        "shell.openInEditor",
        json!({ "cwd": repo.join("does-not-exist.md"), "editor": "file-manager", "reveal": true }),
    )
    .await?;
    let RpcExit::Failure { cause } = reveal_missing else {
        return Err("expected ExternalLauncherRevealTargetMissingError".into());
    };
    assert!(format!("{cause:?}").contains("ExternalLauncherRevealTargetMissingError"));
    server.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn vcs_switch_ref_guards_active_checkout() -> TestResult {
    let (temp, server) = spawn_isolated().await?;
    let repo = temp.path().join("repo");
    init_git_repo(&repo)?;
    let mut ws = WsTestClient::connect(server.addr(), "/ws?orchestrationProtocol=2").await?;

    {
        let conn = server
            .orchestrator()
            .sink()
            .db()
            .lock()
            .map_err(|_| "db poisoned")?;
        ensure_project_store_schema(&conn)?;
        let _ = create_project(
            &*conn,
            &CreateProjectInput::new(repo.to_string_lossy().as_ref())
                .with_project_id("proj-busy")
                .with_title("Repo"),
        )?;
    }
    let cmd = OrchestrationCommand::ThreadCreate(ThreadCreateCommand {
        created_by: OrchestrationActor::User,
        creation_source: CreationSource::Web,
        command_id: CommandId::new("cmd-busy-1")?,
        thread_id: ThreadId::new("thr-busy-1")?,
        project_id: ProjectId::new("proj-busy")?,
        title: "Busy Agent".to_owned(),
        generate_title: None,
        model_selection: ModelSelection {
            instance_id: ProviderInstanceId::new("codex")?,
            model: "gpt-5".to_owned(),
            options: None,
        },
        runtime_mode: RuntimeMode::ApprovalRequired,
        interaction_mode: ProviderInteractionMode::Default,
        branch: None,
        worktree_path: None,
        link_origin: None,
        delegated_from: None,
        imported_native_thread: None,
        initial_message: Some(InitialThreadMessage {
            message_id: MessageId::new("msg-busy-1")?,
            text: "work".to_owned(),
            attachments: Vec::new(),
            context: None,
            title_seed: None,
            scheduled_task_id: None,
            sender_thread_id: None,
            output_schema: None,
            workspace_strategy: None,
        }),
    });
    // Seed the active run without enqueuing a real provider turn that can finish before the RPC.
    let now = "2026-01-01T00:00:00.000Z";
    let decision = decide_command(&DeciderThreadState::default(), &cmd, now)?;
    let mut input = decision.into_commit_input("server", now);
    input.outbox_effects.clear();
    server.orchestrator().sink().commit_command(&input)?;
    let switch_refused = call_ws(
        &mut ws,
        "v3",
        "vcs.switchRef",
        json!({ "cwd": repo, "refName": "other" }),
    )
    .await?;
    let RpcExit::Failure { cause } = switch_refused else {
        return Err("expected the busy checkout to refuse the switch".into());
    };
    assert!(format!("{cause:?}").contains("'Busy Agent' is still working in this checkout"));

    server.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn workflows_definitions_and_local_feedback_rpcs_work() -> TestResult {
    let (temp, server) = spawn_isolated().await?;
    let wf_repo = temp.path().join("wf-scan");
    std::fs::create_dir_all(wf_repo.join(".git"))?;
    let mut ws = WsTestClient::connect(server.addr(), "/ws?orchestrationProtocol=2").await?;

    let scanned = call_ok(
        &mut ws,
        "w0",
        "workspace.scanFolders",
        json!({ "workspaceRoot": temp.path() }),
    )
    .await?;
    assert!(scanned["folders"].is_array());

    let saved = call_ok(
        &mut ws,
        "w1",
        "workflows.saveDefinition",
        json!({
            "name": "sample.ts",
            "source": "Legion.workflow({ name: \"sample\", version: 1, triggers: [], run: () => Effect.gen(function* () { return { ok: true }; }) });"
        }),
    )
    .await?;
    assert_eq!(saved["changed"], true);

    let defs = call_ok(&mut ws, "w2", "workflows.getDefinition", json!({})).await?;
    assert_eq!(defs["files"].as_array().map(Vec::len), Some(1));
    assert!(defs["revision"].is_i64() && defs.get("problem").is_some());

    let deleted = call_ok(
        &mut ws,
        "w3",
        "workflows.deleteDefinition",
        json!({ "name": "sample.ts" }),
    )
    .await?;
    assert_eq!(deleted["changed"], true);

    let bad_scan = call_ws(
        &mut ws,
        "wf-s",
        "workspace.scanFolders",
        json!({ "workspaceRoot": temp.path().join("no-such-dir") }),
    )
    .await?;
    assert!(format!("{bad_scan:?}").contains("WorkspaceFoldersError"));
    assert!(format!("{bad_scan:?}").contains("\"scan\""));

    let err_exit = call_ws(
        &mut ws,
        "af1",
        "agentFeedback.answerRequest",
        json!({ "threadId": "t-1", "turnItemId": "item-1", "answer": "decline" }),
    )
    .await?;
    let RpcExit::Failure { cause } = err_exit else {
        return Err("expected AgentFeedbackRequestError".into());
    };
    assert!(format!("{cause:?}").contains("not_found"));

    server.shutdown().await?;
    Ok(())
}
