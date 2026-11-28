//! Execute/Review settlement and Ship-readiness helpers for E2E-M3.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::support::{CrashableHarness, next_id};
use super::support_board::{
    assert_m3_execute_ship, create_project, get_card, require_m3_execute_ship,
    setup_approved_plan_card,
};
use super::support_m3_mocks::install_mock_providers;
use crate::workflow_board_e2e::TestResult;
use crate::workflow_board_e2e::git_fixtures::create_worktree;
use crate::workflow_board_e2e::ws_rpc::OpaqueWsClient;

pub async fn setup_worktree_plan_card_inner(
    h: &CrashableHarness,
    tag: &str,
    proj: &str,
    title: &str,
) -> TestResult<(OpaqueWsClient, PathBuf, String, i64)> {
    let wt = h.temp_root().join(format!("wt-{tag}"));
    create_worktree(h.workspace_dir(), &wt, &format!("feat/{tag}"))?;
    let tok = h.pair_client(tag, None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, proj, title, h.workspace_dir()).await?;
    let (card_id, rev) = setup_approved_plan_card(&mut ws, proj, &wt, title, None).await?;
    Ok((ws, wt, card_id, rev))
}

pub async fn setup_worktree_plan_card_t1(
    h: &CrashableHarness,
    tag: &str,
    proj: &str,
    title: &str,
) -> TestResult<(OpaqueWsClient, PathBuf, String, i64)> {
    assert_m3_execute_ship(h, tag).await?;
    setup_worktree_plan_card_inner(h, tag, proj, title).await
}

pub async fn setup_worktree_plan_card(
    h: &CrashableHarness,
    tag: &str,
    proj: &str,
    title: &str,
) -> TestResult<Option<(OpaqueWsClient, PathBuf, String, i64)>> {
    if !require_m3_execute_ship(h, tag).await? {
        return Ok(None);
    }
    Ok(Some(
        setup_worktree_plan_card_inner(h, tag, proj, title).await?,
    ))
}

fn rev_of(v: &Value) -> TestResult<i64> {
    v["revision"]
        .as_i64()
        .ok_or_else(|| "missing revision".into())
}

async fn wb(ws: &mut OpaqueWsClient, p: &str, m: &str, body: Value) -> TestResult<Value> {
    ws.call_ok(&next_id(p), m, body).await
}

pub async fn retry_check(
    ws: &mut OpaqueWsClient,
    card_id: &str,
    rev: i64,
    check_id: &str,
) -> TestResult<Value> {
    wb(
        ws,
        "chk",
        "workflowBoard.retryStep",
        json!({
            "cardId": card_id,
            "expectedRevision": rev,
            "stepKind": "check",
            "targetId": check_id,
        }),
    )
    .await
}

pub async fn start_and_settle_card(
    ws: &mut OpaqueWsClient,
    project_id: &str,
    wt_path: &Path,
    title: &str,
    check_argv: Option<Vec<&str>>,
) -> TestResult<(String, i64, Value)> {
    let (card_id, rev) =
        setup_approved_plan_card(ws, project_id, wt_path, title, check_argv).await?;
    let exec_res = start_stage_settled(ws, &card_id, rev, "execute", Some("TSK-1")).await?;
    let _ = retry_check(ws, &card_id, rev_of(&exec_res)?, "CHK-1").await?;
    let card = get_card(ws, &card_id).await?;
    Ok((card_id, card["revision"].as_i64().unwrap_or(rev), card))
}

pub async fn start_stage_settled(
    ws: &mut OpaqueWsClient,
    card_id: &str,
    rev: i64,
    stage: &str,
    task_id: Option<&str>,
) -> TestResult<Value> {
    start_stage_settled_with_model(ws, card_id, rev, stage, task_id, None).await
}

pub async fn await_card_stage_settled(
    ws: &mut OpaqueWsClient,
    card_id: &str,
    stage_run_id: &str,
) -> TestResult<Value> {
    let initial = get_card(ws, card_id).await?;
    let sub_id = next_id("stage-settle");
    ws.subscribe(
        &sub_id,
        "workflowBoard.subscribe",
        json!({ "projectId": initial["projectId"] }),
    )
    .await?;
    loop {
        ws.next_chunk_for(&sub_id).await?;
        let card = get_card(ws, card_id).await?;
        if card["stageRuns"].as_array().is_some_and(|runs| {
            runs.iter().any(|run| {
                run["id"] == stage_run_id
                    && matches!(
                        run["status"].as_str(),
                        Some("completed" | "failed" | "cancelled")
                    )
            })
        }) {
            ws.interrupt(&sub_id).await?;
            return Ok(card);
        }
    }
}

pub async fn start_stage_settled_with_model(
    ws: &mut OpaqueWsClient,
    card_id: &str,
    rev: i64,
    stage: &str,
    task_id: Option<&str>,
    model_selection: Option<Value>,
) -> TestResult<Value> {
    let mut payload = json!({ "cardId": card_id, "expectedRevision": rev, "stage": stage });
    if let Some(tid) = task_id {
        payload["taskId"] = json!(tid);
    }
    if let Some(ms) = model_selection {
        payload["modelSelection"] = ms;
    }
    let mut res = wb(ws, "st", "workflowBoard.startStage", payload).await?;
    let run_id = res["stageRunId"].as_str().ok_or("missing stageRunId")?;
    let card = await_card_stage_settled(ws, card_id, run_id).await?;
    if let Some(post_rev) = card["revision"].as_i64() {
        res["revision"] = json!(post_rev);
    }
    Ok(res)
}

pub async fn execute_and_review_for_ship(
    ws: &mut OpaqueWsClient,
    card_id: &str,
    initial_rev: i64,
) -> TestResult<i64> {
    install_mock_providers(ws, None).await?;
    let before = get_card(ws, card_id).await?;
    let is_findings = before["title"]
        .as_str()
        .is_some_and(|t| t.contains("[REVIEW_FINDING]"));
    let mut cur_rev = initial_rev;
    if let Some(tasks) = before["plan"]["tasks"].as_array() {
        for task in tasks {
            if let Some(tid) = task["taskId"].as_str() {
                let r = start_stage_settled(ws, card_id, cur_rev, "execute", Some(tid)).await?;
                cur_rev = rev_of(&r)?;
            }
        }
    } else {
        let r = start_stage_settled(ws, card_id, cur_rev, "execute", None).await?;
        cur_rev = rev_of(&r)?;
    }
    if let Some(checks) = before["plan"]["checks"].as_array() {
        for chk in checks {
            if let Some(cid) = chk["id"].as_str() {
                let r = retry_check(ws, card_id, cur_rev, cid).await?;
                cur_rev = rev_of(&r)?;
            }
        }
    }
    let instance_id = if is_findings {
        "mock-review-findings"
    } else {
        "mock-review-pass"
    };
    let rev_res = start_stage_settled_with_model(
        ws,
        card_id,
        cur_rev,
        "review",
        None,
        Some(json!({ "instanceId": instance_id, "model": "claude-sonnet-4-5" })),
    )
    .await?;
    rev_of(&rev_res)
}
