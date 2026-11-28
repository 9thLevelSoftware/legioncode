//! E2E-M5 Tier 3 pairwise Rust CLI tests (`t3_r07`..`t3_r12`).

use std::fs;
use std::path::Path;

use serde_json::{Value, json};

use crate::workflow_board_e2e::TestResult;
use crate::workflow_board_e2e::git_fixtures::{
    CliLockKind, CopyLegionCliOpts, copy_disposable_legion_cli, create_worktree,
    seed_active_cli_lock,
};

use super::helpers::{
    assert_auth_error, assert_cli_import_card_fields, assert_domain_failure, connect_ws_at,
    create_and_approve_m1_spec, create_project, mint_scoped_session, probe_m4_cli_import,
    probe_mcp_board_tools, run_git, start_m1_ws, ws_call, ws_ok,
};
use super::helpers_m3::{
    complete_m2_plan, install_provider, install_reviewer, run_check, settle_stage,
    start_crash_m3_if_ready, start_m3_ws, start_stage, start_stage_settled,
};

fn has_receipt_file_under(dir: &Path) -> TestResult<bool> {
    if !dir.exists() {
        return Ok(false);
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry
            .file_name()
            .to_string_lossy()
            .to_ascii_lowercase()
            .contains("receipt")
        {
            return Ok(true);
        }
        if path.is_dir() && has_receipt_file_under(&path)? {
            return Ok(true);
        }
    }
    Ok(false)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t3_r07_f14_f15_out_of_contract_edits_preserved_and_amended() -> TestResult<()> {
    let (h, _, mut ws, wt, cid, plan_rev) = start_m3_ws("t3-r07", "outcomes")
        .await?
        .ok_or("required workflow board scenario setup")?;
    let secret = wt.join("src").join("unexpected.rs");
    let model = install_provider(
        &mut ws, h.workspace_dir().parent().ok_or("missing parent")?,
        "scenario-executor", "Completed task.",
        r#"const fs = await import('node:fs'); fs.mkdirSync('src', {recursive: true}); fs.writeFileSync('src/unexpected.rs', 'pub fn unexpected() {}');"#,
    ).await?;
    let (_, blocked) =
        start_stage_settled(&mut ws, &cid, &json!(plan_rev), "execute", Some(model)).await?;
    assert_eq!(blocked["status"], "blocked");
    assert!(
        blocked["fileContractReports"][0]["unexpectedFiles"]
            .as_array()
            .ok_or("missing unexpectedFiles")?
            .contains(&json!("src/unexpected.rs"))
    );
    assert_eq!(
        blocked["fileContractReports"][0]["preservedInWorktree"],
        true
    );
    assert_eq!(fs::read_to_string(&secret)?, "pub fn unexpected() {}");
    let amended = ws_ok(
        &mut ws,
        "workflowBoard.proposeAmendment",
        json!({
            "cardId": cid,
            "expectedRevision": blocked["revision"],
            "targetStage": "plan",
            "reason": "Allow the preserved unexpected edit",
            "sourceTaskId": "TSK-0001",
            "unexpectedFiles": ["src/unexpected.rs"],
            "tasks": [{
                "taskId": "TSK-0001", "title": "Core", "dependsOn": [],
                "filesAllowed": ["src/core.rs", "src/unexpected.rs"],
                "filesForbidden": ["src/secret.rs"], "testingMethod": "test-first",
            }],
        }),
    )
    .await?;
    let approved = ws_ok(
        &mut ws,
        "workflowBoard.approvePlan",
        json!({ "cardId": cid, "expectedRevision": amended["revision"] }),
    )
    .await?;
    let (_, executed) =
        start_stage_settled(&mut ws, &cid, &approved["revision"], "execute", None).await?;
    assert_eq!(executed["taskExecutions"][0]["status"], "completed");
    assert_eq!(fs::read_to_string(&secret)?, "pub fn unexpected() {}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t3_r08_f16_f20_argv_check_scrubbed_env_and_host_receipts() -> TestResult<()> {
    let (mut srv, mut ws) = start_crash_m3_if_ready(
        &[
            ("LEGIONCODE_DEV_AUTH_TOKEN", "dev-secret-e2e"),
            ("OPENAI_API_KEY", "sk-secret-e2e"),
        ],
        "t3_r08",
    )
    .await?
    .ok_or("required workflow board scenario setup")?;
    let wt = srv.temp_dir.path().join("wt-t3-r08");
    create_worktree(&srv.workspace_dir, &wt, "feat/t3-r08")?;
    create_project(&mut ws, "proj-t3-r08", "T3 R08", &srv.workspace_dir).await?;
    let (cid, spec_rev, _) =
        create_and_approve_m1_spec(&mut ws, "proj-t3-r08", &wt, "Scrubbed Env").await?;
    let script = srv.temp_dir.path().join("check-env.cjs");
    fs::write(
        &script,
        "if (Object.keys(process.env).some(k => k.startsWith('LEGIONCODE_')) || process.env.OPENAI_API_KEY) process.exit(2);",
    )?;
    let plan_rev = complete_m2_plan(
        &mut ws,
        &cid,
        spec_rev,
        false,
        "risk-first",
        Some(vec![
            "node",
            script.to_str().ok_or("non-UTF-8 script path")?,
        ]),
    )
    .await?;
    let (_, executed) =
        start_stage_settled(&mut ws, &cid, &json!(plan_rev), "execute", None).await?;
    let card = run_check(&mut ws, &executed).await?;
    assert_eq!(card["checkReceipts"][0]["status"], "passed");
    assert!(card["checkReceipts"][0]["productSnapshotDigest"].is_string());
    assert!(!has_receipt_file_under(
        &wt.join(".legion").join("workflows").join(&cid)
    )?);

    let tbl_count: i64 = srv.open_db_readonly()?.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name LIKE 'workflow_board_%'",
        [],
        |r| r.get(0),
    )?;
    assert!(tbl_count > 0);

    drop(ws);
    srv.restart()?;
    let mut ws2 = srv.pair_and_connect("t3-r08-post").await?;
    let persisted = ws_ok(&mut ws2, "workflowBoard.getCard", json!({ "cardId": cid })).await?;
    assert_eq!(persisted["checkReceipts"][0]["status"], "passed");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t3_r09_f17_f21_fresh_review_thread_and_card_lifetime_vs_cancel() -> TestResult<()> {
    let (h, _, mut ws, _wt, cid, plan_rev) = start_m3_ws("t3-r09", "outcomes")
        .await?
        .ok_or("required workflow board scenario setup")?;
    let (exec, executed) =
        start_stage_settled(&mut ws, &cid, &json!(plan_rev), "execute", None).await?;
    let checked = run_check(&mut ws, &executed).await?;
    let root = h.workspace_dir().parent().ok_or("missing parent")?;
    let gate = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let addr = gate.local_addr()?;
    let model = install_provider(&mut ws, root, "scenario-cancel-review", "unreachable",
        &format!("const net = await import('node:net'); await new Promise(resolve => net.connect({}, '127.0.0.1').on('close', resolve));", addr.port()),
    ).await?;
    let cancelled = start_stage(&mut ws, &cid, &checked["revision"], "review", Some(model)).await?;
    let (connection, _) =
        tokio::time::timeout(std::time::Duration::from_secs(10), gate.accept()).await??;
    ws_ok(
        &mut ws,
        "workflows.cancel",
        json!({ "runId": cancelled["runId"], "reason": "Cancel review job" }),
    )
    .await?;
    let job = ws_ok(
        &mut ws,
        "workflows.get",
        json!({ "runId": cancelled["runId"] }),
    )
    .await?;
    assert_eq!(job["run"]["status"], "cancelled");
    let after_cancel = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        settle_stage(&mut ws, &cancelled),
    )
    .await??;
    drop(connection);
    assert!(
        after_cancel["stageRuns"]
            .as_array()
            .ok_or("missing stageRuns")?
            .iter()
            .any(|run| run["id"] == cancelled["stageRunId"] && run["status"] == "failed")
    );
    assert_eq!(after_cancel["cardId"], cid);
    let reviewer = install_reviewer(&mut ws, root).await?;
    let (review, reviewed) = start_stage_settled(
        &mut ws,
        &cid,
        &after_cancel["revision"],
        "review",
        Some(reviewer),
    )
    .await?;
    assert_ne!(review["threadId"], exec["threadId"]);
    assert_ne!(review["threadId"], cancelled["threadId"]);
    assert_eq!(reviewed["reviewReport"]["threadId"], review["threadId"]);
    assert_eq!(reviewed["reviewReport"]["verdict"], "pass");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t3_r10_f18_f23_multi_client_ship_delivery_and_agent_rejection() -> TestResult<()> {
    let (h, tok_a, mut ws_a, _wt, cid, plan_rev) = start_m3_ws("t3-r10", "outcomes")
        .await?
        .ok_or("required workflow board scenario setup")?;
    let (exec, executed) =
        start_stage_settled(&mut ws_a, &cid, &json!(plan_rev), "execute", None).await?;
    let checked = run_check(&mut ws_a, &executed).await?;
    assert_eq!(checked["checkReceipts"][0]["status"], "passed");
    let root = h.workspace_dir().parent().ok_or("missing parent")?;
    let reviewer = install_reviewer(&mut ws_a, root).await?;
    let (review, reviewed) = start_stage_settled(
        &mut ws_a,
        &cid,
        &checked["revision"],
        "review",
        Some(reviewer),
    )
    .await?;
    assert_ne!(review["threadId"], exec["threadId"]);
    assert_eq!(reviewed["reviewReport"]["verdict"], "pass");

    let tok_b = h.pair_client("t3-r10-b", None).await?;
    let mut ws_b = connect_ws_at(h.addr()?, &tok_b).await?;
    let client_b_card = ws_ok(&mut ws_b, "workflowBoard.getCard", json!({ "cardId": cid })).await?;
    assert_eq!(client_b_card["revision"], reviewed["revision"]);
    let cur_rev = &client_b_card["revision"];
    let agent_tok = mint_scoped_session(
        h.origin()?,
        &tok_a,
        "mcp-oauth:r10",
        &["orchestration:operate"],
    )
    .await?;
    let mut agent_ws = connect_ws_at(h.addr()?, &agent_tok).await?;
    let denied = ws_call(
        &mut agent_ws,
        "workflowBoard.approveDelivery",
        json!({ "cardId": cid, "expectedRevision": cur_rev }),
    )
    .await?;
    assert_auth_error(
        &denied,
        "orchestration:operate",
        "client:interactive",
        "approveDelivery",
    )?;
    let acc = ws_ok(
        &mut ws_b,
        "workflowBoard.recordManualAcceptance",
        json!({
            "cardId": cid,
            "expectedRevision": cur_rev,
            "criterionId": "AC-02",
            "verdict": "pass",
            "note": "Client B verified",
        }),
    )
    .await?;
    let _ = ws_ok(
        &mut ws_b,
        "workflowBoard.approveDelivery",
        json!({ "cardId": cid, "expectedRevision": acc["revision"] }),
    )
    .await?;
    let shipped = ws_ok(&mut ws_a, "workflowBoard.getCard", json!({ "cardId": cid })).await?;
    assert_eq!(shipped["stage"], "ship");
    assert_eq!(shipped["status"], "completed");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t3_r11_f14_f22_scoped_mcp_board_tools_card_isolation() -> TestResult<()> {
    let (h, tok, _) = start_m1_ws("t3_r11")
        .await?
        .ok_or("required workflow board scenario setup")?;
    assert!(
        probe_mcp_board_tools(&h, &tok, "t3_r11").await?,
        "required workflow board capability"
    );
    let resp = h
        .post_mcp(
            Some(&tok),
            &[("x-legioncode-thread-id", "thr-card-a")],
            &json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {
                    "name": "legion_workflow_board_read_brief",
                    "arguments": { "cardId": "card-b" },
                },
            }),
        )
        .await?;
    let body: Value = resp.json().await?;
    assert_eq!(body["result"]["isError"], true);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t3_r12_f20_f24_cli_import_provenance_no_autocommit_and_lock_guard() -> TestResult<()> {
    let (h, _, mut ws) = start_m1_ws("t3_r12")
        .await?
        .ok_or("required workflow board scenario setup")?;
    assert!(
        probe_m4_cli_import(&mut ws, "t3_r12").await?,
        "required workflow board capability"
    );
    create_project(&mut ws, "proj-t3-r12", "T3 R12", h.workspace_dir()).await?;
    let _ = copy_disposable_legion_cli(h.workspace_dir(), CopyLegionCliOpts::default())?;
    assert!(
        !h.workspace_dir().join(".legion-cli").join("cache").exists(),
        "missing gitignored cache/ directory must be tolerated"
    );
    let lock = seed_active_cli_lock(h.workspace_dir(), CliLockKind::EngineLock)?;
    let head_before = run_git(h.workspace_dir(), &["rev-parse", "HEAD"])?;
    let refused = ws_call(
        &mut ws,
        "workflowBoard.applyCliImport",
        json!({ "projectId": "proj-t3-r12" }),
    )
    .await?;
    assert_domain_failure(
        &refused,
        &["active", "lock", "running", "cli", "refus"],
        "t3_r12 EngineLock refusal",
    )?;
    fs::remove_file(lock)?;
    let applied = ws_ok(
        &mut ws,
        "workflowBoard.applyCliImport",
        json!({ "projectId": "proj-t3-r12" }),
    )
    .await?;
    assert!(applied.to_string().contains("spec-t3-code"));
    let cid = applied["cardId"].as_str().ok_or("missing cardId")?;
    let detail = ws_ok(&mut ws, "workflowBoard.getCard", json!({ "cardId": cid })).await?;
    assert_cli_import_card_fields(&detail)?;
    assert_eq!(
        run_git(h.workspace_dir(), &["rev-parse", "HEAD"])?,
        head_before
    );
    Ok(())
}
