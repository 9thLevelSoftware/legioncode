//! E2E-M5 Tier 4 scenarios S1, S2, S3, and S7a (persistence, launch, guided UI, non-UI, journal).

use std::fs;
use std::path::Path;

use serde_json::json;

use crate::workflow_board_e2e::TestResult;
use crate::workflow_board_e2e::git_fixtures::{commit_file, create_worktree};
use crate::workflow_board_e2e::harness::OpaqueBoxHarness;

use super::helpers::{
    connect_ws_at, create_and_approve_m1_spec, create_project, next_id, probe_m3_execute_ship,
    probe_m3_git_delivery, require_m1_board, run_git, ws_ok,
};
use super::helpers_m3::{
    CrashServer, await_thread_settled, complete_m2_plan, enable_workflows, install_reviewer,
    run_check, start_stage_settled,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t4_s01_m1_one_handoff_across_host_restart() -> TestResult<()> {
    let mut h = OpaqueBoxHarness::start(true)?;
    assert!(
        require_m1_board(&h, "t4_s01").await?,
        "required workflow board capability"
    );
    let wt = h
        .workspace_dir()
        .parent()
        .ok_or("missing parent")?
        .join("wt-t4-s01");
    create_worktree(h.workspace_dir(), &wt, "feat/t4-s01")?;
    let head_before = run_git(&wt, &["rev-parse", "HEAD"])?;

    let tok1 = h.pair_client("t4-s01-a", None).await?;
    let mut ws1 = connect_ws_at(h.addr()?, &tok1).await?;
    let _ = ws_ok(
        &mut ws1,
        "server.updateSettings",
        json!({ "patch": { "workflowsEnabled": true } }),
    )
    .await?;
    create_project(
        &mut ws1,
        "proj-t4-s01",
        "Scenario 1 Rust",
        h.workspace_dir(),
    )
    .await?;
    let (cid, _, digest) =
        create_and_approve_m1_spec(&mut ws1, "proj-t4-s01", &wt, "S1 Rust Card").await?;
    drop(ws1);

    h.restart_server()?;
    let tok2 = h.pair_client("t4-s01-b", None).await?;
    let mut ws2 = connect_ws_at(h.addr()?, &tok2).await?;
    let detail = ws_ok(&mut ws2, "workflowBoard.getCard", json!({ "cardId": cid })).await?;
    assert_eq!(detail["specApprovalStatus"], "valid");
    assert_eq!(detail["specApproval"]["specDigest"], digest);
    let rev_post = detail["revision"].as_i64().ok_or("missing rev")?;

    let launch1 = ws_ok(
        &mut ws2,
        "workflowBoard.startStage",
        json!({ "cardId": cid, "expectedRevision": rev_post, "stage": "plan" }),
    )
    .await?;
    assert_eq!(launch1["resumed"], false);
    let tid = launch1["threadId"].as_str().ok_or("missing threadId")?;
    let rev_after_start = launch1["revision"].as_i64().ok_or("missing rev")?;

    let launch2 = ws_ok(
        &mut ws2,
        "workflowBoard.startStage",
        json!({ "cardId": cid, "expectedRevision": rev_after_start, "stage": "plan" }),
    )
    .await?;
    assert_eq!(launch2["resumed"], true);
    assert_eq!(launch2["threadId"], tid);

    let proj = ws_ok(
        &mut ws2,
        "orchestration.getThreadProjection",
        json!({ "threadId": tid }),
    )
    .await?;
    assert_eq!(
        proj["thread"]["worktreePath"].as_str(),
        Some(wt.to_string_lossy().as_ref())
    );
    let settled = await_thread_settled(&mut ws2, tid).await?;
    assert!(
        settled["runs"]
            .as_array()
            .ok_or("missing runs")?
            .iter()
            .any(|run| run["status"] == "completed")
    );
    let brief = ws_ok(
        &mut ws2,
        "workflowBoard.getArtifact",
        json!({ "cardId": cid, "key": "plan-brief.md" }),
    )
    .await?;
    assert!(
        brief["content"]
            .as_str()
            .is_some_and(|s| s.contains("AC-01"))
    );
    assert_eq!(run_git(&wt, &["rev-parse", "HEAD"])?, head_before);
    Ok(())
}

async fn execute_review_and_ship(
    ws: &mut crate::workflow_board_e2e::ws_rpc::OpaqueWsClient,
    root: &Path,
    cid: &str,
    plan_rev: i64,
    ac02_status: &str,
    observation: &str,
) -> TestResult<()> {
    let (exec, executed) = start_stage_settled(ws, cid, &json!(plan_rev), "execute", None).await?;
    assert_eq!(executed["taskExecutions"][0]["status"], "completed");
    let checked = run_check(ws, &executed).await?;
    assert_eq!(checked["checkReceipts"][0]["status"], "passed");
    let reviewer = install_reviewer(ws, root).await?;
    let (review, after_rev) =
        start_stage_settled(ws, cid, &checked["revision"], "review", Some(reviewer)).await?;
    assert_ne!(review["threadId"], exec["threadId"]);
    assert_eq!(after_rev["reviewReport"]["verdict"], "pass");
    let acc = ws_ok(
        ws,
        "workflowBoard.recordManualAcceptance",
        json!({
            "cardId": cid,
            "expectedRevision": after_rev["revision"],
            "criterionId": "AC-02",
            "verdict": ac02_status,
            "note": observation,
        }),
    )
    .await?;
    let _ = ws_ok(
        ws,
        "workflowBoard.approveDelivery",
        json!({ "cardId": cid, "expectedRevision": acc["revision"] }),
    )
    .await?;
    let shipped = ws_ok(ws, "workflowBoard.getCard", json!({ "cardId": cid })).await?;
    assert_eq!(shipped["stage"], "ship");
    assert_eq!(shipped["status"], "completed");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t4_s02_greenfield_ui_feature_guided_wireframes_to_ship() -> TestResult<()> {
    let h = OpaqueBoxHarness::start(true)?;
    assert!(
        require_m1_board(&h, "t4_s02").await?,
        "required workflow board capability"
    );
    let tok = h.pair_client("t4-s02", None).await?;
    let mut ws = connect_ws_at(h.addr()?, &tok).await?;
    enable_workflows(&mut ws).await?;
    let temp_parent = h.workspace_dir().parent().ok_or("missing parent")?;
    assert!(
        probe_m3_git_delivery(&mut ws, temp_parent, "t4_s02").await?,
        "required workflow board capability"
    );
    let wt = temp_parent.join("wt-t4-s02");
    create_worktree(h.workspace_dir(), &wt, "feat/t4-s02")?;
    let head_before = run_git(&wt, &["rev-parse", "HEAD"])?;
    create_project(&mut ws, "proj-t4-s02", "Scenario 2 UI", h.workspace_dir()).await?;

    let created = ws_ok(
        &mut ws,
        "workflowBoard.createCard",
        json!({
            "projectId": "proj-t4-s02",
            "title": "Guided UI Card",
            "brief": "Interactive filter panel with wireframe.",
            "sourceType": "idea",
            "assistanceMode": "guided",
            "workspaceStrategy": { "type": "existing_worktree", "worktreePath": wt.to_string_lossy() },
            "modelSelection": { "instanceId": "devReplay", "model": "dev-replay" },
            "runtimeMode": "approval-required",
            "interactionMode": "default",
        }),
    )
    .await?;
    let cid = created["cardId"].as_str().ok_or("missing cardId")?;
    let wf_path = wt
        .join(".legion")
        .join("workflows")
        .join(cid)
        .join("wireframes")
        .join("index.html");
    if let Some(parent) = wf_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        &wf_path,
        "<!doctype html><html><body><h1>Filter</h1></body></html>\n",
    )?;

    let paused = ws_ok(
        &mut ws,
        "workflowBoard.exploreStep",
        json!({ "cardId": cid, "expectedRevision": created["revision"], "action": "pause", "answers": [{ "decisionId": "D-1", "choice": "recommend" }] }),
    )
    .await?;
    let resumed = ws_ok(
        &mut ws,
        "workflowBoard.exploreStep",
        json!({ "cardId": cid, "expectedRevision": paused["revision"], "action": "resume" }),
    )
    .await?;
    let spec_up = ws_ok(
        &mut ws,
        "workflowBoard.updateSpec",
        json!({ "cardId": cid, "expectedRevision": resumed["revision"], "acceptanceCriteria": [{ "id": "AC-01", "text": "Filter updates board view." }, { "id": "AC-02", "text": "Visual wireframe layout verified." }], "wireframesIndex": "wireframes/index.html" }),
    )
    .await?;
    let chall = ws_ok(
        &mut ws,
        "workflowBoard.challengeStep",
        json!({ "cardId": cid, "expectedRevision": spec_up["revision"], "action": "complete", "concerns": [] }),
    )
    .await?;
    let spec_ok = ws_ok(
        &mut ws,
        "workflowBoard.approveSpec",
        json!({ "cardId": cid, "expectedRevision": chall["revision"] }),
    )
    .await?;
    assert!(spec_ok["specApproval"]["wireframesDigest"].is_string());

    let plan_rev = complete_m2_plan(
        &mut ws,
        cid,
        spec_ok["revision"].as_i64().ok_or("missing rev")?,
        true,
        "outcomes",
        None,
    )
    .await?;
    execute_review_and_ship(
        &mut ws,
        temp_parent,
        cid,
        plan_rev,
        "pass",
        "UI wireframe verified",
    )
    .await?;
    assert_eq!(run_git(&wt, &["rev-parse", "HEAD"])?, head_before);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t4_s03_non_ui_backend_feature_without_wireframes() -> TestResult<()> {
    let h = OpaqueBoxHarness::start(true)?;
    assert!(
        require_m1_board(&h, "t4_s03").await?,
        "required workflow board capability"
    );
    let tok = h.pair_client("t4-s03", None).await?;
    let mut ws = connect_ws_at(h.addr()?, &tok).await?;
    enable_workflows(&mut ws).await?;
    assert!(
        probe_m3_execute_ship(&mut ws, "t4_s03").await?,
        "required workflow board capability"
    );
    let _ = commit_file(
        h.workspace_dir(),
        "docs/backend-brief.md",
        "# Backend Brief\n\nNo UI needed.\n",
        "add backend brief",
    )?;
    let wt = h
        .workspace_dir()
        .parent()
        .ok_or("missing parent")?
        .join("wt-t4-s03");
    create_worktree(h.workspace_dir(), &wt, "feat/t4-s03")?;
    create_project(
        &mut ws,
        "proj-t4-s03",
        "Scenario 3 Backend",
        h.workspace_dir(),
    )
    .await?;

    let created = ws_ok(
        &mut ws,
        "workflowBoard.createCard",
        json!({
            "projectId": "proj-t4-s03",
            "title": "Non-UI Backend",
            "briefPath": "docs/backend-brief.md",
            "sourceType": "file",
            "assistanceMode": "direct",
            "workspaceStrategy": { "type": "existing_worktree", "worktreePath": wt.to_string_lossy() },
            "modelSelection": { "instanceId": "devReplay", "model": "dev-replay" },
            "runtimeMode": "approval-required",
            "interactionMode": "default",
        }),
    )
    .await?;
    let cid = created["cardId"].as_str().ok_or("missing cardId")?;
    let updated = ws_ok(
        &mut ws,
        "workflowBoard.updateSpec",
        json!({
            "cardId": cid,
            "expectedRevision": created["revision"],
            "acceptanceCriteria": [
                { "id": "AC-01", "text": "Backend runs cleanly." },
                { "id": "AC-02", "text": "No wireframe inspection required." },
            ],
            "wireframesIndex": serde_json::Value::Null,
        }),
    )
    .await?;
    let approved = ws_ok(
        &mut ws,
        "workflowBoard.approveSpec",
        json!({ "cardId": cid, "expectedRevision": updated["revision"] }),
    )
    .await?;
    assert!(approved["specApproval"]["wireframesDigest"].is_null());
    let plan_rev = complete_m2_plan(
        &mut ws,
        cid,
        approved["revision"].as_i64().ok_or("missing rev")?,
        false,
        "risk-first",
        None,
    )
    .await?;
    execute_review_and_ship(
        &mut ws,
        h.workspace_dir().parent().ok_or("missing parent")?,
        cid,
        plan_rev,
        "not-applicable",
        "Backend-only card without UI.",
    )
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t4_s07a_write_journal_crash_recovery_and_external_edit_active() -> TestResult<()> {
    let mut h = CrashServer::start()?;
    let wt = h.temp_dir.path().join("wt-t4-s07a");
    create_worktree(&h.workspace_dir, &wt, "feat/t4-s07a")?;
    let mut ws1 = h.pair_and_connect("t4-s07a-a").await?;
    create_project(&mut ws1, "proj-t4-s07a", "Scenario 7a", &h.workspace_dir).await?;
    let (cid, _, _) =
        create_and_approve_m1_spec(&mut ws1, "proj-t4-s07a", &wt, "Crash Recovery").await?;
    drop(ws1);

    let card_dir = wt.join(".legion").join("workflows").join(&cid);
    let spec_path = card_dir.join("spec.md");
    let committed_spec = fs::read_to_string(&spec_path)?;
    fs::write(
        &spec_path,
        format!("{committed_spec}\n<!-- manual external edit -->\n"),
    )?;
    h.inject_torn_journal_and_restart(&card_dir, &cid, "staged")?;

    let mut ws2 = h.pair_and_connect("t4-s07a-b").await?;
    let reconciled = ws2
        .call_ok(
            &next_id("gc"),
            "workflowBoard.getCard",
            json!({ "cardId": cid }),
        )
        .await?;
    assert_eq!(reconciled["specApprovalStatus"], "stale");
    Ok(())
}
