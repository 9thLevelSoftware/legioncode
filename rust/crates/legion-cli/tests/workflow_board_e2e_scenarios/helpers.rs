//! Shared opaque-box probes, WS/HTTP helpers, and M1 card builders for E2E-M5.

use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use legion_protocol::rpc_codec::{RpcCauseReason, RpcExit};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde_json::{Value, json};

use crate::workflow_board_e2e::TestResult;
use crate::workflow_board_e2e::harness::OpaqueBoxHarness;
use crate::workflow_board_e2e::ws_rpc::OpaqueWsClient;

static SEQ: AtomicU64 = AtomicU64::new(1);

pub(crate) fn next_id(prefix: &str) -> String {
    format!("{prefix}-{}", SEQ.fetch_add(1, Ordering::Relaxed))
}

pub(crate) async fn connect_ws_at(
    addr: std::net::SocketAddr,
    token: &str,
) -> TestResult<OpaqueWsClient> {
    let hdr = format!("Bearer {token}");
    OpaqueWsClient::connect(
        addr,
        "/ws?orchestrationProtocol=2&frameParts=1",
        &[("Authorization", &hdr)],
    )
    .await
}

pub(crate) async fn mint_scoped_session(
    origin: &str,
    admin_tok: &str,
    subject: &str,
    scopes: &[&str],
) -> TestResult<String> {
    let resp = reqwest::Client::new()
        .post(format!("{origin}/api/auth/sessions"))
        .header(AUTHORIZATION, format!("Bearer {admin_tok}"))
        .header(CONTENT_TYPE, "application/json")
        .json(&json!({ "subject": subject, "label": format!("e2e-{subject}"), "scopes": scopes }))
        .send()
        .await?;
    if resp.status() != 201 {
        return Err(format!("POST /api/auth/sessions returned {}", resp.status()).into());
    }
    let body: Value = resp.json().await?;
    Ok(body["token"].as_str().ok_or("missing token")?.to_owned())
}

pub(crate) async fn ws_ok(ws: &mut OpaqueWsClient, method: &str, body: Value) -> TestResult<Value> {
    ws.call_ok(&next_id("ok"), method, body).await
}

pub(crate) async fn ws_call(
    ws: &mut OpaqueWsClient,
    method: &str,
    body: Value,
) -> TestResult<RpcExit> {
    ws.call_unary(&next_id("call"), method, body).await
}

pub(crate) fn assert_auth_error(
    exit: &RpcExit,
    scope: &str,
    perm: &str,
    ctx: &str,
) -> TestResult<()> {
    let RpcExit::Failure { cause } = exit else {
        return Err(format!("{ctx}: expected auth failure").into());
    };
    let Some(RpcCauseReason::Fail { error }) = cause.first() else {
        return Err(format!("{ctx}: unexpected failure cause {cause:?}").into());
    };
    assert_eq!(error["_tag"], "EnvironmentAuthorizationError", "{ctx}");
    assert_eq!(error["requiredScope"], scope, "{ctx}");
    assert_eq!(error["requiredPermission"], perm, "{ctx}");
    Ok(())
}

pub(crate) fn err_tag_and_code(exit: &RpcExit) -> (Option<&str>, Option<&str>) {
    match exit {
        RpcExit::Failure { cause } => match cause.first() {
            Some(RpcCauseReason::Fail { error }) => {
                (error["_tag"].as_str(), error["code"].as_str())
            }
            _ => (None, None),
        },
        RpcExit::Success { .. } => (None, None),
    }
}

pub(crate) fn assert_domain_failure(
    exit: &RpcExit,
    expected_keywords: &[&str],
    ctx: &str,
) -> TestResult<()> {
    let RpcExit::Failure { cause } = exit else {
        return Err(format!("{ctx}: expected domain failure, got success").into());
    };
    let Some(RpcCauseReason::Fail { error }) = cause.first() else {
        return Err(format!("{ctx}: expected Fail cause, got {cause:?}").into());
    };
    let tag = error["_tag"].as_str().unwrap_or("");
    if tag == "UnsupportedRpcMethod" || tag == "ParseError" {
        return Err(format!("{ctx}: unexpected {tag} instead of domain error: {error}").into());
    }
    let text = error.to_string().to_ascii_lowercase();
    if !expected_keywords.is_empty() && !expected_keywords.iter().any(|kw| text.contains(kw)) {
        return Err(format!("{ctx}: expected one of {expected_keywords:?} in {error}").into());
    }
    Ok(())
}

pub(crate) async fn require_m1_board(h: &OpaqueBoxHarness, name: &str) -> TestResult<bool> {
    let env_desc = h.probe_environment().await?;
    if env_desc["capabilities"]["projectWorkflowBoard"].as_bool() == Some(true) {
        return Ok(true);
    }
    eprintln!("SKIPPED {name}: capabilities.projectWorkflowBoard is false");
    Ok(false)
}

pub(crate) type M1Ws = (OpaqueBoxHarness, String, OpaqueWsClient);

pub(crate) async fn start_m1_ws(tag: &str) -> TestResult<Option<M1Ws>> {
    let h = OpaqueBoxHarness::start(true)?;
    if !require_m1_board(&h, tag).await? {
        return Ok(None);
    }
    let tok = h.pair_client(tag, None).await?;
    let ws = connect_ws_at(h.addr()?, &tok).await?;
    Ok(Some((h, tok, ws)))
}

async fn probe_rpc_supported(
    ws: &mut OpaqueWsClient,
    method: &str,
    body: Value,
    label: &str,
    name: &str,
) -> TestResult<bool> {
    let e = ws_call(ws, method, body).await?;
    let ready = err_tag_and_code(&e).0 != Some("UnsupportedRpcMethod");
    if !ready {
        eprintln!("SKIPPED {name}: {label} not yet implemented");
    }
    Ok(ready)
}

pub(crate) async fn probe_m2_planning(ws: &mut OpaqueWsClient, name: &str) -> TestResult<bool> {
    probe_rpc_supported(
        ws,
        "workflowBoard.updatePlan",
        json!({ "cardId": "__m2__", "expectedRevision": 1, "plan": {} }),
        "M2 planning",
        name,
    )
    .await
}

pub(crate) async fn probe_m3_execute_ship(ws: &mut OpaqueWsClient, name: &str) -> TestResult<bool> {
    if !probe_m2_planning(ws, name).await? {
        return Ok(false);
    }
    for method in ["workflowBoard.pauseCard", "workflowBoard.approveDelivery"] {
        if !probe_rpc_supported(
            ws,
            method,
            json!({ "cardId": "__m3__", "expectedRevision": 1 }),
            "M3 execute/ship",
            name,
        )
        .await?
        {
            return Ok(false);
        }
    }
    let e = ws_call(
        ws,
        "workflowBoard.startStage",
        json!({ "cardId": "__m3__", "expectedRevision": 1, "stage": "execute" }),
    )
    .await?;
    let (tag, code) = err_tag_and_code(&e);
    let ready = tag != Some("UnsupportedRpcMethod") && code != Some("invalid_stage_transition");
    if !ready {
        eprintln!("SKIPPED {name}: M3 execute/ship not yet implemented");
    }
    Ok(ready)
}

pub(crate) async fn probe_m3_git_delivery(
    ws: &mut OpaqueWsClient,
    temp_root: &Path,
    name: &str,
) -> TestResult<bool> {
    if !probe_m3_execute_ship(ws, name).await? {
        return Ok(false);
    }
    let sub_id = next_id("git-probe");
    ws.subscribe(
        &sub_id,
        "git.runStackedAction",
        json!({
            "actionId": next_id("act"),
            "cwd": temp_root.join("__git_probe__").to_string_lossy(),
            "action": "push"
        }),
    )
    .await?;
    let stubbed = loop {
        let chunk = ws.next_chunk_for(&sub_id).await?;
        let kind = chunk["kind"].as_str().unwrap_or("");
        if kind == "action_finished" || kind == "action_failed" {
            break chunk["result"]["push"]["status"] == "skipped_not_requested";
        }
    };
    ws.interrupt(&sub_id).await?;
    if stubbed {
        eprintln!("SKIPPED {name}: M3 git push/create_pr not yet implemented");
    }
    Ok(!stubbed)
}

pub(crate) async fn probe_m4_cli_import(ws: &mut OpaqueWsClient, name: &str) -> TestResult<bool> {
    probe_rpc_supported(
        ws,
        "workflowBoard.previewCliImport",
        json!({ "projectId": "__m4__" }),
        "M4 CLI import",
        name,
    )
    .await
}

pub(crate) async fn probe_mcp_board_tools(
    h: &OpaqueBoxHarness,
    tok: &str,
    name: &str,
) -> TestResult<bool> {
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {} });
    let resp = h
        .post_mcp(Some(tok), &[("x-legioncode-thread-id", "thr-probe")], &body)
        .await?;
    let text = resp.json::<Value>().await?.to_string();
    let ready = text.contains("legion_workflow_board_read_brief")
        && text.contains("legion_workflow_board_propose");
    if !ready {
        eprintln!("SKIPPED {name}: scoped board MCP tools not yet implemented");
    }
    Ok(ready)
}

pub(crate) async fn create_project(
    ws: &mut OpaqueWsClient,
    pid: &str,
    title: &str,
    root: &Path,
) -> TestResult<()> {
    let body = json!({
        "type": "project.create",
        "commandId": next_id("cmd-proj"),
        "projectId": pid,
        "title": title,
        "workspaceRoot": root.to_string_lossy(),
    });
    let _ = ws_ok(ws, "orchestration.dispatchCommand", body).await?;
    Ok(())
}

pub(crate) async fn create_and_approve_m1_spec(
    ws: &mut OpaqueWsClient,
    pid: &str,
    wt: &Path,
    title: &str,
) -> TestResult<(String, i64, String)> {
    let mut create_body: Value = serde_json::from_str(
        r#"{"sourceType":"idea","assistanceMode":"balanced","modelSelection":{"instanceId":"devReplay","model":"dev-replay"},"runtimeMode":"approval-required","interactionMode":"default"}"#,
    )?;
    create_body["projectId"] = json!(pid);
    create_body["title"] = json!(title);
    create_body["brief"] = json!(format!("Deliver {title}."));
    create_body["workspaceStrategy"] =
        json!({ "type": "existing_worktree", "worktreePath": wt.to_string_lossy() });
    let created = ws_ok(ws, "workflowBoard.createCard", create_body).await?;
    let cid = created["cardId"]
        .as_str()
        .ok_or("missing cardId")?
        .to_owned();
    let rev1 = created["revision"].as_i64().ok_or("missing rev1")?;
    let spec_body = json!({
        "cardId": cid,
        "expectedRevision": rev1,
        "acceptanceCriteria": [
            { "id": "AC-01", "text": "Worktree deliverables persist." },
            { "id": "AC-02", "text": "Approvals bind to digest." }
        ],
        "constraints": ["Zero auto-commits"],
        "sourceRefs": ["README.md"],
        "wireframesIndex": Value::Null,
    });
    let updated = ws_ok(ws, "workflowBoard.updateSpec", spec_body).await?;
    let rev2 = updated["revision"].as_i64().ok_or("missing rev2")?;
    let approved = ws_ok(
        ws,
        "workflowBoard.approveSpec",
        json!({ "cardId": cid, "expectedRevision": rev2 }),
    )
    .await?;
    let rev3 = approved["revision"].as_i64().ok_or("missing rev3")?;
    let digest = approved["specApproval"]["specDigest"]
        .as_str()
        .unwrap_or("")
        .to_owned();
    Ok((cid, rev3, digest))
}

pub(crate) fn run_git(dir: &Path, args: &[&str]) -> TestResult<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "")
        .output()?;
    if !out.status.success() {
        return Err(format!("git failed: {}", String::from_utf8_lossy(&out.stderr)).into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

pub(crate) fn assert_cli_import_card_fields(detail: &Value) -> TestResult<()> {
    let tasks = detail["tasks"].as_array().ok_or("missing tasks array")?;
    assert_eq!(tasks.len(), 8, "expected 8 tasks in imported CLI card");
    let tsk2 = tasks
        .iter()
        .find(|t| t["id"] == "TSK-0002" || t["taskId"] == "TSK-0002")
        .ok_or("missing TSK-0002")?;
    assert_eq!(tsk2["status"], "blocked");
    let s = detail.to_string();
    assert!(s.contains("packages/provider-mimo/qualification/cli.test.mjs"));
    assert!(s.contains("verify-1.log"));
    assert!(s.contains("passScore"));
    assert!(s.contains("requireHardened"));
    assert!(s.contains("trustKeys"));
    Ok(())
}
