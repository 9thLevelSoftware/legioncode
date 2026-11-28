//! RPC, thread-settlement, git-action, and workflow-board setup helpers for E2E-M3.

use std::path::Path;

use legion_protocol::rpc_codec::{RpcCauseReason, RpcExit};
use serde_json::{Value, json};

use super::support::{CrashableHarness, next_id};
use crate::workflow_board_e2e::TestResult;
use crate::workflow_board_e2e::ws_rpc::OpaqueWsClient;

pub async fn require_board(h: &CrashableHarness, test_name: &str) -> TestResult<bool> {
    let env_desc = h.probe_environment().await?;
    if env_desc["capabilities"]["projectWorkflowBoard"].as_bool() == Some(true) {
        return Ok(true);
    }
    eprintln!("SKIPPED {test_name}: capabilities.projectWorkflowBoard is not enabled");
    Ok(false)
}

fn err_tag_and_code(res: &RpcExit) -> (Option<&str>, Option<&str>) {
    match res {
        RpcExit::Failure { cause } => match cause.first() {
            Some(RpcCauseReason::Fail { error }) => {
                (error["_tag"].as_str(), error["code"].as_str())
            }
            _ => (None, None),
        },
        RpcExit::Success { .. } => (None, None),
    }
}

pub async fn probe_m3_update_plan_ws(ws: &mut OpaqueWsClient) -> TestResult<bool> {
    let res = ws
        .call_unary(
            &next_id("m3-plan"),
            "workflowBoard.updatePlan",
            json!({ "cardId": "__m3_probe__", "expectedRevision": 1, "plan": {} }),
        )
        .await?;
    Ok(err_tag_and_code(&res).0 != Some("UnsupportedRpcMethod"))
}

pub async fn probe_m3_execute_ship_ws(ws: &mut OpaqueWsClient) -> TestResult<bool> {
    if !probe_m3_update_plan_ws(ws).await? {
        return Ok(false);
    }
    let res = ws
        .call_unary(
            &next_id("m3-exec"),
            "workflowBoard.startStage",
            json!({ "cardId": "__m3_probe__", "expectedRevision": 1, "stage": "execute" }),
        )
        .await?;
    let (tag, code) = err_tag_and_code(&res);
    Ok(tag != Some("UnsupportedRpcMethod") && code != Some("invalid_stage_transition"))
}

async fn connect_for_probe(
    h: &CrashableHarness,
    test_name: &str,
) -> TestResult<Option<OpaqueWsClient>> {
    if !require_board(h, test_name).await? {
        return Ok(None);
    }
    let tok = h.pair_client("m3-probe", None).await?;
    Ok(Some(h.connect_ws(&tok).await?))
}

pub async fn require_m3_update_plan(h: &CrashableHarness, test_name: &str) -> TestResult<bool> {
    let Some(mut ws) = connect_for_probe(h, test_name).await? else {
        return Ok(false);
    };
    let ready = probe_m3_update_plan_ws(&mut ws).await?;
    if !ready {
        eprintln!("SKIPPED {test_name}: workflowBoard.updatePlan not yet implemented");
    }
    Ok(ready)
}

pub async fn require_m3_execute_ship(h: &CrashableHarness, test_name: &str) -> TestResult<bool> {
    let Some(mut ws) = connect_for_probe(h, test_name).await? else {
        return Ok(false);
    };
    let ready = probe_m3_execute_ship_ws(&mut ws).await?;
    if !ready {
        eprintln!("SKIPPED {test_name}: M3 Execute/Ship RPCs not yet implemented");
    }
    Ok(ready)
}

pub async fn require_m3_git_delivery(h: &CrashableHarness, test_name: &str) -> TestResult<bool> {
    let Some(mut ws) = connect_for_probe(h, test_name).await? else {
        return Ok(false);
    };
    let cwd = h.temp_root().join("__m3_git_probe__");
    let payload =
        json!({ "actionId": next_id("m3-git"), "cwd": cwd.to_string_lossy(), "action": "push" });
    let chunks = run_stacked_git_action(&mut ws, payload).await?;
    let stubbed = chunks.iter().any(|c| {
        c["kind"] == "action_finished" && c["result"]["push"]["status"] == "skipped_not_requested"
    });
    if stubbed {
        eprintln!("SKIPPED {test_name}: M3 git.runStackedAction push/pr/filePaths not implemented");
    }
    Ok(!stubbed)
}

pub async fn assert_m3_execute_ship(h: &CrashableHarness, test_name: &str) -> TestResult<()> {
    let ready = require_m3_execute_ship(h, test_name).await?;
    assert!(
        ready,
        "{test_name}: capabilities.projectWorkflowBoard and M3 Execute/Ship RPCs must be enabled"
    );
    Ok(())
}

pub async fn assert_m3_git_delivery(h: &CrashableHarness, test_name: &str) -> TestResult<()> {
    let ready = require_m3_git_delivery(h, test_name).await?;
    assert!(
        ready,
        "{test_name}: M3 git.runStackedAction push/pr/filePaths must be enabled"
    );
    Ok(())
}

fn rev_of(v: &Value) -> TestResult<i64> {
    v["revision"]
        .as_i64()
        .ok_or_else(|| "missing revision".into())
}

async fn wb(ws: &mut OpaqueWsClient, p: &str, m: &str, body: Value) -> TestResult<Value> {
    ws.call_ok(&next_id(p), m, body).await
}

pub async fn get_card(ws: &mut OpaqueWsClient, card_id: &str) -> TestResult<Value> {
    wb(
        ws,
        "get",
        "workflowBoard.getCard",
        json!({ "cardId": card_id }),
    )
    .await
}

pub async fn create_project(
    ws: &mut OpaqueWsClient,
    project_id: &str,
    title: &str,
    workspace_root: &Path,
) -> TestResult<()> {
    let _ = ws.call_ok(&next_id("proj"), "orchestration.dispatchCommand", json!({ "type": "project.create", "commandId": next_id("cmd-proj"), "projectId": project_id, "title": title, "workspaceRoot": workspace_root.to_string_lossy() })).await?;
    Ok(())
}

pub async fn await_thread_settled(ws: &mut OpaqueWsClient, thread_id: &str) -> TestResult<Value> {
    let sub_id = next_id("sub-thread");
    ws.subscribe(
        &sub_id,
        "orchestration.subscribeThread",
        json!({ "threadId": thread_id, "afterSequence": 0, "acceptBoundedSnapshot": true }),
    )
    .await?;
    loop {
        let proj = wb(
            ws,
            "get-proj",
            "orchestration.getThreadProjection",
            json!({ "threadId": thread_id }),
        )
        .await?;
        let settled = proj["runs"].as_array().is_some_and(|runs| {
            runs.iter().any(|r| {
                matches!(
                    r["status"].as_str(),
                    Some("completed" | "failed" | "interrupted")
                )
            })
        });
        if settled {
            ws.interrupt(&sub_id).await?;
            return Ok(proj);
        }
        let _ = ws.next_chunk_for(&sub_id).await?;
    }
}

pub async fn run_stacked_git_action(
    ws: &mut OpaqueWsClient,
    payload: Value,
) -> TestResult<Vec<Value>> {
    let sub_id = next_id("git-act");
    ws.subscribe(&sub_id, "git.runStackedAction", payload)
        .await?;
    let mut chunks = Vec::new();
    loop {
        let chunk = ws.next_chunk_for(&sub_id).await?;
        let done = matches!(
            chunk["kind"].as_str(),
            Some("action_finished" | "action_failed")
        );
        chunks.push(chunk);
        if done {
            break;
        }
    }
    Ok(chunks)
}

pub async fn expect_rpc_err_tag(
    ws: &mut OpaqueWsClient,
    tag: &str,
    payload: Value,
    expected_tag: &str,
) -> TestResult<Value> {
    expect_rpc_err_any_tag(ws, tag, payload, &[expected_tag]).await
}

pub async fn expect_rpc_err_any_tag(
    ws: &mut OpaqueWsClient,
    tag: &str,
    payload: Value,
    expected_tags: &[&str],
) -> TestResult<Value> {
    match ws.call_unary(&next_id("err"), tag, payload).await? {
        RpcExit::Failure { cause } => {
            if let Some(RpcCauseReason::Fail { error }) = cause.into_iter().next() {
                if error["_tag"]
                    .as_str()
                    .is_some_and(|t| expected_tags.contains(&t))
                {
                    return Ok(error);
                }
                return Err(format!("expected _tag in {expected_tags:?}, got {error}").into());
            }
            Err(format!("{tag} failed without Fail cause").into())
        }
        RpcExit::Success { value } => {
            Err(format!("expected {tag} to fail with {expected_tags:?}, got {value}").into())
        }
    }
}

pub use super::support_board_ship::{
    execute_and_review_for_ship, retry_check, start_and_settle_card, start_stage_settled,
    start_stage_settled_with_model,
};

pub async fn setup_approved_plan_card(
    ws: &mut OpaqueWsClient,
    project_id: &str,
    wt_path: &Path,
    title: &str,
    check_argv: Option<Vec<&str>>,
) -> TestResult<(String, i64)> {
    let argv: Vec<&str> = check_argv.unwrap_or_else(|| vec!["git", "status", "--porcelain"]);
    setup_approved_plan_card_with_checks(
        ws, project_id, wt_path, title,
        vec![json!({ "id": "CHK-1", "criterionId": "AC-1", "argv": argv })],
        json!([
            { "criterionId": "AC-1", "method": "automated", "checkIds": ["CHK-1"] },
            { "criterionId": "AC-2", "method": "manual", "procedure": "Verify manually in worktree." }
        ]),
    ).await
}

pub async fn setup_spec_approved_card(
    ws: &mut OpaqueWsClient,
    project_id: &str,
    wt_path: &Path,
    title: &str,
    constraints: Vec<&str>,
    source_refs: Vec<&str>,
) -> TestResult<(String, i64)> {
    let _ = wb(
        ws,
        "wb-enable-wf",
        "server.updateSettings",
        json!({ "patch": { "workflowsEnabled": true } }),
    )
    .await?;
    let created = wb(ws, "wb-create", "workflowBoard.createCard", json!({
        "projectId": project_id, "title": title,
        "workspaceStrategy": { "type": "existing_worktree", "worktreePath": wt_path.to_string_lossy() },
        "modelSelection": { "instanceId": "devReplay", "model": "dev-replay" },
        "runtimeMode": "auto-accept-edits",
    })).await?;
    let card_id = created["cardId"]
        .as_str()
        .ok_or("missing cardId")?
        .to_owned();
    let spec_up = wb(ws, "wb-spec", "workflowBoard.updateSpec", json!({
        "cardId": card_id, "expectedRevision": rev_of(&created)?, "brief": "Execute/Ship test card",
        "acceptanceCriteria": [{ "id": "AC-1", "text": "Automated check passes" }, { "id": "AC-2", "text": "Manual verification recorded" }],
        "constraints": constraints, "sourceRefs": source_refs,
    })).await?;
    let spec_ok = wb(
        ws,
        "wb-app-spec",
        "workflowBoard.approveSpec",
        json!({ "cardId": card_id, "expectedRevision": rev_of(&spec_up)? }),
    )
    .await?;
    Ok((card_id, rev_of(&spec_ok)?))
}

pub async fn setup_approved_plan_card_with_checks(
    ws: &mut OpaqueWsClient,
    project_id: &str,
    wt_path: &Path,
    title: &str,
    checks: Vec<Value>,
    criterion_mappings: Value,
) -> TestResult<(String, i64)> {
    let (card_id, rev3) =
        setup_spec_approved_card(ws, project_id, wt_path, title, vec![], vec![]).await?;
    let plan_payload = super::support_m3_mocks::valid_plan_update_payload(
        &card_id,
        rev3,
        super::support_m3_mocks::valid_plan_tasks(),
        Value::Array(checks),
        criterion_mappings,
    );
    let plan_up = wb(ws, "wb-plan", "workflowBoard.updatePlan", plan_payload).await?;
    let plan_ok = wb(
        ws,
        "wb-app-plan",
        "workflowBoard.approvePlan",
        json!({ "cardId": card_id, "expectedRevision": rev_of(&plan_up)? }),
    )
    .await?;
    Ok((card_id, rev_of(&plan_ok)?))
}

pub fn launch_wt_payload(
    project_id: &str,
    title: &str,
    wt: &Path,
    branch: Option<&str>,
    stable_key: Option<&str>,
    reuse: bool,
) -> Value {
    json!({
        "commandId": next_id("cmd"),
        "projectId": project_id,
        "title": title,
        "stableLaunchKey": stable_key,
        "reuseExistingThread": reuse,
        "modelSelection": { "instanceId": "devReplay", "model": "dev-replay" },
        "runtimeMode": "approval-required",
        "interactionMode": "plan",
        "workspaceStrategy": {
            "type": "existing_worktree",
            "worktreePath": wt.to_string_lossy(),
            "branch": branch,
        },
    })
}

pub async fn launch_replay_wt(
    ws: &mut OpaqueWsClient,
    project_id: &str,
    title: &str,
    wt: &Path,
    branch: Option<&str>,
    stable_key: Option<&str>,
    reuse: bool,
) -> TestResult<Value> {
    ws.call_ok(
        &next_id("launch"),
        "orchestration.launchThread",
        launch_wt_payload(project_id, title, wt, branch, stable_key, reuse),
    )
    .await
}
