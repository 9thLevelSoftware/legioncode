use std::process::Command as StdCommand;
use std::time::{SystemTime, UNIX_EPOCH};

use super::ws_client::WsTestClient;
use legion_cli::serve::{RunningServer, ServeConfig, ServeMode, spawn_server};
use legion_persistence::pairing_tokens::{
    IssuePairingTokenInput, STANDARD_CLIENT_SCOPES, issue_pairing_token_with_conn,
};
use legion_persistence::project_store::{
    CreateProjectInput, create_project, ensure_project_store_schema,
};
use legion_persistence::sqlite::SqliteStore;
use legion_protocol::rpc_codec::{
    ClientRpcFrame, RpcExit, RpcId, ServerRpcFrame, decode_server_frames, encode_client_frame,
};
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn std::error::Error>>;

async fn call_ok(
    ws: &mut WsTestClient,
    id: &str,
    tag: &str,
    payload: Value,
) -> Result<Value, Box<dyn std::error::Error>> {
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
        for resp in decode_server_frames(&ws.recv_text().await?)? {
            if let ServerRpcFrame::Exit { request_id, exit } = resp
                && request_id == RpcId::from(id)
            {
                return match exit {
                    RpcExit::Success { value } => Ok(value),
                    RpcExit::Failure { cause } => Err(format!("{tag} failed: {cause:?}").into()),
                };
            }
        }
    }
}

async fn boot(base: &std::path::Path) -> Result<RunningServer, Box<dyn std::error::Error>> {
    spawn_server(ServeConfig {
        base_dir: base.to_path_buf(),
        base_dir_is_explicit: true,
        port: 0,
        host: Some("127.0.0.1".to_owned()),
        mode: ServeMode::Web,
        no_browser: true,
        lan: false,
        static_dir: None,
        cwd: base.to_path_buf(),
    })
    .await
    .map_err(Into::into)
}

async fn pair_client(
    server: &RunningServer,
    repo_dir: &std::path::Path,
) -> Result<String, Box<dyn std::error::Error>> {
    let now_ms = i64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis())?;
    let credential = {
        let store = SqliteStore::open(server.paths().db_path.clone())?;
        ensure_project_store_schema(store.connection())?;
        create_project(
            store.connection(),
            &CreateProjectInput::new(&*repo_dir.to_string_lossy())
                .with_project_id("proj-host-m2")
                .with_title("Host M2"),
        )?;
        issue_pairing_token_with_conn(
            store.connection(),
            &IssuePairingTokenInput {
                scopes: STANDARD_CLIENT_SCOPES
                    .iter()
                    .map(|s| (*s).to_owned())
                    .collect(),
                ..IssuePairingTokenInput::default()
            },
            now_ms,
        )?
        .credential
    };
    let resp = reqwest::Client::new()
        .post(format!("{}/api/auth/pair", server.origin()))
        .json(&json!({ "token": credential, "clientLabel": "host-m2-client" }))
        .send()
        .await?;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await?;
    Ok(body["accessToken"]
        .as_str()
        .ok_or("missing accessToken")?
        .to_owned())
}

async fn drive_spec_explore_challenge(
    ws: &mut WsTestClient,
    cid: &str,
    rev1: u64,
) -> Result<(u64, String), Box<dyn std::error::Error>> {
    let started_spec = call_ok(
        ws,
        "ss1",
        "workflowBoard.startStage",
        json!({ "cardId": cid, "expectedRevision": rev1, "stage": "spec", "modelSelection": { "instanceId": "devReplay", "model": "dev-replay" } }),
    )
    .await?;
    assert_eq!(started_spec["stage"], "spec");
    let mut rev = started_spec["revision"].as_u64().ok_or("rev")?;
    let upd_spec = call_ok(
        ws,
        "us1",
        "workflowBoard.updateSpec",
        json!({ "cardId": cid, "expectedRevision": rev, "acceptanceCriteria": [{ "id": "AC-01", "text": "Survives restart" }] }),
    )
    .await?;
    rev = upd_spec["revision"].as_u64().ok_or("rev")?;
    let spec_digest = upd_spec["specDigest"]
        .as_str()
        .ok_or("specDigest")?
        .to_owned();

    let mut exp = call_ok(
        ws,
        "ex1",
        "workflowBoard.exploreStep",
        json!({ "cardId": cid, "expectedRevision": rev, "action": "step" }),
    )
    .await?;
    rev = exp["revision"].as_u64().ok_or("rev")?;
    for round in 0..6 {
        let unres: Vec<Value> = exp["decisions"].as_array().map_or_default(|arr| {
            arr.iter()
                .filter(|d| d["status"] == "unresolved")
                .map(|d| json!({ "decisionId": d["id"], "action": "recommend" }))
                .collect()
        });
        if unres.is_empty() {
            break;
        }
        exp = call_ok(
            ws,
            &format!("exr{round}"),
            "workflowBoard.exploreStep",
            json!({ "cardId": cid, "expectedRevision": rev, "action": "step", "continueRound": true, "answers": unres }),
        )
        .await?;
        rev = exp["revision"].as_u64().ok_or("rev")?;
    }
    let ch1 = call_ok(
        ws,
        "ch1",
        "workflowBoard.challengeStep",
        json!({ "cardId": cid, "expectedRevision": rev, "action": "start", "concerns": [{ "id": "c-host-1", "summary": "Confirm restart", "disposition": "unresolved" }] }),
    )
    .await?;
    rev = ch1["revision"].as_u64().ok_or("rev")?;
    let ch2 = call_ok(
        ws,
        "ch2",
        "workflowBoard.challengeStep",
        json!({ "cardId": cid, "expectedRevision": rev, "action": "resolve", "concerns": [{ "id": "c-host-1", "summary": "Confirm restart", "disposition": "answer", "answer": "Persisted in SQLite" }] }),
    )
    .await?;
    rev = ch2["revision"].as_u64().ok_or("rev")?;
    let app_spec = call_ok(
        ws,
        "as1",
        "workflowBoard.approveSpec",
        json!({ "cardId": cid, "expectedRevision": rev, "expectedDigest": spec_digest }),
    )
    .await?;
    Ok((app_spec["revision"].as_u64().ok_or("rev")?, spec_digest))
}

async fn drive_plan_approve_and_return(
    ws: &mut WsTestClient,
    cid: &str,
    rev: u64,
    spec_digest: &str,
) -> TestResult {
    let upd_plan = call_ok(
        ws,
        "up1",
        "workflowBoard.updatePlan",
        json!({
            "cardId": cid,
            "expectedRevision": rev,
            "lenses": [
                { "lens": "ux", "applicable": false, "reason": "N/A" },
                { "lens": "functional", "applicable": true, "summary": "AC-01" },
                { "lens": "architecture", "applicable": true, "summary": "Clean" },
                { "lens": "quality", "applicable": true, "summary": "Tested" },
                { "lens": "infrastructure", "applicable": false, "reason": "N/A" },
                { "lens": "delivery", "applicable": true, "summary": "Single step" }
            ],
            "alternatives": [
                { "id": "a1", "title": "A1", "summary": "S1", "tradeoffs": "T1" },
                { "id": "a2", "title": "A2", "summary": "S2", "tradeoffs": "T2" }
            ],
            "selectedAlternativeId": "a1",
            "selectionRationale": "Simple",
            "strategy": "outcomes",
            "granularity": "balanced",
            "tasks": [{ "taskId": "TSK-01", "title": "Check", "filesAllowed": ["README.md"], "verificationCommands": [["git", "status"]], "testingMethod": "regression-first" }],
            "checks": [{ "id": "CHK-01", "argv": ["git", "status"], "description": "Git check" }],
            "acMappings": [{ "criterionId": "AC-01", "method": "automated", "checkIds": ["CHK-01"] }]
        }),
    )
    .await?;
    let plan_rev = upd_plan["revision"].as_u64().ok_or("rev")?;
    let plan_digest = upd_plan["planDigest"]
        .as_str()
        .ok_or("planDigest")?
        .to_owned();

    let app_plan = call_ok(ws, "ap1", "workflowBoard.approvePlan", json!({ "cardId": cid, "expectedRevision": plan_rev, "expectedSpecDigest": spec_digest, "expectedPlanDigest": plan_digest })).await?;
    let app_rev = app_plan["revision"].as_u64().ok_or("rev")?;
    let ret = call_ok(ws, "ret1", "workflowBoard.returnForRevision", json!({ "cardId": cid, "expectedRevision": app_rev, "targetStage": "plan", "reason": "Re-verify" })).await?;
    assert_eq!(ret["specApproval"]["status"], "valid");
    assert_eq!(ret["planApproval"]["status"], "stale");
    Ok(())
}

#[tokio::test]
async fn host_restart_preserves_m2_explore_challenge_plan_and_return_for_revision() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let base = tmp.path().join("host-m2");
    let repo_dir = tmp.path().join("repo-m2");
    std::fs::create_dir_all(&repo_dir)?;
    for args in [
        vec!["init"],
        vec!["config", "user.email", "test@legion.local"],
        vec!["config", "user.name", "Test"],
    ] {
        StdCommand::new("git")
            .current_dir(&repo_dir)
            .args(args)
            .output()?;
    }
    std::fs::write(repo_dir.join("README.md"), "# M2 Host Repo\n")?;
    StdCommand::new("git")
        .current_dir(&repo_dir)
        .args(["add", "."])
        .output()?;
    StdCommand::new("git")
        .current_dir(&repo_dir)
        .args(["commit", "-m", "init"])
        .output()?;

    let server = boot(&base).await?;
    let tok = pair_client(&server, &repo_dir).await?;
    let mut ws = WsTestClient::connect(
        server.addr(),
        &format!("/ws?orchestrationProtocol=2&wsTicket={tok}"),
    )
    .await?;
    call_ok(
        &mut ws,
        "cfg",
        "server.updateSettings",
        json!({ "patch": { "workflowsEnabled": true } }),
    )
    .await?;

    let created = call_ok(
        &mut ws,
        "c1",
        "workflowBoard.createCard",
        json!({ "projectId": "proj-host-m2", "title": "M2 Host Card", "brief": "Host persistence", "sourceType": "idea", "assistanceMode": "balanced" }),
    )
    .await?;
    let cid = created["cardId"].as_str().ok_or("cardId")?.to_owned();
    let rev1 = created["revision"].as_u64().ok_or("rev")?;
    let (rev, spec_digest) = drive_spec_explore_challenge(&mut ws, &cid, rev1).await?;
    drive_plan_approve_and_return(&mut ws, &cid, rev, &spec_digest).await?;

    server.shutdown().await?;
    let server2 = boot(&base).await?;
    let mut ws2 = WsTestClient::connect(
        server2.addr(),
        &format!("/ws?orchestrationProtocol=2&wsTicket={tok}"),
    )
    .await?;
    let detail = call_ok(
        &mut ws2,
        "g1",
        "workflowBoard.getCard",
        json!({ "cardId": cid, "projectId": "proj-host-m2" }),
    )
    .await?;
    assert_eq!(detail["stage"], "plan");
    assert_eq!(detail["planApproval"]["status"], "stale");
    assert_eq!(detail["tasks"][0]["taskId"], "TSK-01");
    server2.shutdown().await?;
    Ok(())
}
