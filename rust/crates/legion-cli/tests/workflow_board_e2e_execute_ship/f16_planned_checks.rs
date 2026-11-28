//! F16: Planned checks & snapshot-bound receipts (5 Tier 1 + 5 Tier 2).

use std::fs;

use serde_json::json;

use super::support::CrashableHarness;
use super::support_board::{
    assert_m3_execute_ship, create_project, expect_rpc_err_any_tag, expect_rpc_err_tag, get_card,
    require_m3_execute_ship, retry_check, setup_approved_plan_card,
    setup_approved_plan_card_with_checks, setup_spec_approved_card, start_and_settle_card,
    start_stage_settled,
};
use crate::workflow_board_e2e::TestResult;
use crate::workflow_board_e2e::git_fixtures::create_worktree;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f16_t1_1_planned_argv_check_runs_in_card_worktree_and_binds_snapshot() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    assert_m3_execute_ship(
        &h,
        "f16_t1_1_planned_argv_check_runs_in_card_worktree_and_binds_snapshot",
    )
    .await?;
    let wt = h.temp_root().join("wt-f16-pass");
    create_worktree(h.workspace_dir(), &wt, "feat/f16-pass")?;
    let tok = h.pair_client("f16-t1-1", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f16-1", "F16 Pass", h.workspace_dir()).await?;
    let (_, _, card) = start_and_settle_card(
        &mut ws,
        "proj-f16-1",
        &wt,
        "F16 Pass Card",
        Some(vec!["git", "rev-parse", "--is-inside-work-tree"]),
    )
    .await?;
    assert_eq!(card["checkReceipts"][0]["status"], "passed");
    assert!(card["checkReceipts"][0]["productSnapshotDigest"].is_string());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f16_t1_2_failing_planned_check_blocks_card_with_exit_and_log_pointer() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    assert_m3_execute_ship(
        &h,
        "f16_t1_2_failing_planned_check_blocks_card_with_exit_and_log_pointer",
    )
    .await?;
    let wt = h.temp_root().join("wt-f16-fail");
    create_worktree(h.workspace_dir(), &wt, "feat/f16-fail")?;
    let tok = h.pair_client("f16-t1-2", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f16-2", "F16 Fail", h.workspace_dir()).await?;
    let (_, _, card) = start_and_settle_card(
        &mut ws,
        "proj-f16-2",
        &wt,
        "F16 Fail Card",
        Some(vec![
            "git",
            "rev-parse",
            "--verify",
            "refs/heads/does-not-exist",
        ]),
    )
    .await?;
    assert_eq!(card["checkReceipts"][0]["status"], "failed");
    assert!(
        card["checkReceipts"][0]["exitCode"]
            .as_i64()
            .is_some_and(|c| c != 0)
    );
    assert!(card["checkReceipts"][0]["logRef"].is_string());
    assert!(
        card["shipReadiness"]["blockers"]
            .as_array()
            .is_some_and(|b| b.iter().any(|x| x["kind"] == "check_failed"))
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f16_t1_3_worktree_edit_changes_snapshot_and_stales_prior_receipt() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    assert_m3_execute_ship(
        &h,
        "f16_t1_3_worktree_edit_changes_snapshot_and_stales_prior_receipt",
    )
    .await?;
    let wt = h.temp_root().join("wt-f16-stale");
    create_worktree(h.workspace_dir(), &wt, "feat/f16-stale")?;
    let tok = h.pair_client("f16-t1-3", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f16-3", "F16 Stale", h.workspace_dir()).await?;
    let (card_id, _, _) = start_and_settle_card(
        &mut ws,
        "proj-f16-3",
        &wt,
        "F16 Stale Card",
        Some(vec!["git", "status", "--porcelain"]),
    )
    .await?;
    fs::write(wt.join("README.md"), "# Snapshot S2\n")?;
    let card = get_card(&mut ws, &card_id).await?;
    assert_eq!(card["checkReceipts"][0]["status"], "stale");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f16_t1_4_host_receipts_survive_restart_and_stay_out_of_workspace() -> TestResult<()> {
    let mut h = CrashableHarness::start(true)?;
    assert_m3_execute_ship(
        &h,
        "f16_t1_4_host_receipts_survive_restart_and_stay_out_of_workspace",
    )
    .await?;
    let wt = h.temp_root().join("wt-f16-host");
    create_worktree(h.workspace_dir(), &wt, "feat/f16-host")?;
    let tok = h.pair_client("f16-t1-4", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f16-4", "F16 Host", h.workspace_dir()).await?;
    let (card_id, _, _) =
        start_and_settle_card(&mut ws, "proj-f16-4", &wt, "F16 Host Card", None).await?;
    drop(ws);

    h.restart_server()?;
    let tok2 = h.pair_client("f16-t1-4b", None).await?;
    let mut ws2 = h.connect_ws(&tok2).await?;
    let card = get_card(&mut ws2, &card_id).await?;
    assert!(
        card["checkReceipts"]
            .as_array()
            .is_some_and(|c| !c.is_empty())
    );
    assert!(
        !wt.join(".legion")
            .join("workflows")
            .join(&card_id)
            .join("receipts.json")
            .exists()
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f16_t1_5_multiple_checks_track_independent_criterion_states() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    assert_m3_execute_ship(
        &h,
        "f16_t1_5_multiple_checks_track_independent_criterion_states",
    )
    .await?;
    let wt = h.temp_root().join("wt-f16-multi");
    create_worktree(h.workspace_dir(), &wt, "feat/f16-multi")?;
    let tok = h.pair_client("f16-t1-5", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f16-5", "F16 Multi", h.workspace_dir()).await?;
    let checks = vec![
        json!({ "id": "CHK-1", "criterionId": "AC-1", "argv": ["node", "-e", "process.exit(0)"] }),
        json!({ "id": "CHK-2", "criterionId": "AC-2", "argv": ["node", "-e", "process.exit(1)"] }),
    ];
    let mappings = json!([
        { "criterionId": "AC-1", "method": "automated", "checkIds": ["CHK-1"] },
        { "criterionId": "AC-2", "method": "automated", "checkIds": ["CHK-2"] },
    ]);
    let (card_id, rev) = setup_approved_plan_card_with_checks(
        &mut ws,
        "proj-f16-5",
        &wt,
        "F16 Multi Card",
        checks,
        mappings,
    )
    .await?;
    let exec = start_stage_settled(&mut ws, &card_id, rev, "execute", None).await?;
    let c1 = retry_check(
        &mut ws,
        &card_id,
        exec["revision"].as_i64().unwrap_or(rev + 1),
        "CHK-1",
    )
    .await?;
    let _ = retry_check(
        &mut ws,
        &card_id,
        c1["revision"].as_i64().unwrap_or(rev + 2),
        "CHK-2",
    )
    .await?;
    let card = get_card(&mut ws, &card_id).await?;
    let receipts = card["checkReceipts"]
        .as_array()
        .ok_or("missing checkReceipts")?;
    assert_eq!(
        receipts
            .iter()
            .find(|r| r["checkId"] == "CHK-1")
            .and_then(|r| r["status"].as_str()),
        Some("passed")
    );
    assert_eq!(
        receipts
            .iter()
            .find(|r| r["checkId"] == "CHK-2")
            .and_then(|r| r["status"].as_str()),
        Some("failed")
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f16_t2_1_board_get_card_rejected_cleanly_when_capability_disabled() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    let env_desc = h.probe_environment().await?;
    let tok = h.pair_client("f16-t2-1", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    if env_desc["capabilities"]["projectWorkflowBoard"].as_bool() == Some(true) {
        let err = expect_rpc_err_any_tag(
            &mut ws,
            "workflowBoard.getCard",
            json!({ "cardId": "card-16" }),
            &["WorkflowBoardRequestError"],
        )
        .await?;
        assert_eq!(err["code"], "card_not_found");
    } else {
        let _ = expect_rpc_err_tag(
            &mut ws,
            "workflowBoard.getCard",
            json!({ "cardId": "card-16" }),
            "UnsupportedRpcMethod",
        )
        .await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f16_t2_2_shell_metacharacter_injection_rejected_as_non_argv() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    assert!(
        require_m3_execute_ship(
            &h,
            "f16_t2_2_shell_metacharacter_injection_rejected_as_non_argv",
        )
        .await?,
        "required workflow board capability"
    );
    let wt = h.temp_root().join("wt-f16-inj");
    create_worktree(h.workspace_dir(), &wt, "feat/f16-inj")?;
    let tok = h.pair_client("f16-t2-2", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f16-6", "F16 Inj", h.workspace_dir()).await?;
    let (card_id, rev) =
        setup_spec_approved_card(&mut ws, "proj-f16-6", &wt, "F16 Inj Card", vec![], vec![])
            .await?;
    let err = expect_rpc_err_tag(&mut ws, "workflowBoard.updatePlan", json!({
        "cardId": card_id, "expectedRevision": rev,
        "checks": [{ "id": "CHK-1", "criterionId": "AC-1", "argv": ["node", "-e", "; echo pwned"] }],
    }), "ParseError").await?;
    assert_eq!(
        err["message"],
        "expected a valid argv-only command without shell wrappers or control operators"
    );
    assert_eq!(get_card(&mut ws, &card_id).await?["revision"], rev);
    assert!(!wt.join("pwned").exists());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f16_t2_3_credential_env_vars_scrubbed_from_check_child() -> TestResult<()> {
    let h = CrashableHarness::start_with_env(
        true,
        &[
            ("OPENAI_API_KEY", "sk-secret-e2e"),
            ("ANTHROPIC_API_KEY", "ant-secret-e2e"),
            ("LEGIONCODE_DEV_AUTH_TOKEN", "dev-secret-e2e"),
        ],
    )?;
    assert!(
        require_m3_execute_ship(&h, "f16_t2_3_credential_env_vars_scrubbed_from_check_child")
            .await?,
        "required workflow board capability"
    );
    let wt = h.temp_root().join("wt-f16-scrub");
    create_worktree(h.workspace_dir(), &wt, "feat/f16-scrub")?;
    let tok = h.pair_client("f16-t2-3", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f16-7", "F16 Scrub", h.workspace_dir()).await?;
    let script = wt.join("check-env.cjs");
    fs::write(
        &script,
        "process.exit(process.env.OPENAI_API_KEY || process.env.ANTHROPIC_API_KEY || process.env.LEGIONCODE_DEV_AUTH_TOKEN ? 2 : 0)\n",
    )?;
    let (_, _, card) = start_and_settle_card(
        &mut ws,
        "proj-f16-7",
        &wt,
        "F16 Scrub Card",
        Some(vec!["node", "check-env.cjs"]),
    )
    .await?;
    assert_eq!(card["checkReceipts"][0]["status"], "passed");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f16_t2_4_snapshot_s1_to_s2_edit_marks_receipt_stale_until_rerun() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    assert!(
        require_m3_execute_ship(
            &h,
            "f16_t2_4_snapshot_s1_to_s2_edit_marks_receipt_stale_until_rerun",
        )
        .await?,
        "required workflow board capability"
    );
    let wt = h.temp_root().join("wt-f16-s12");
    create_worktree(h.workspace_dir(), &wt, "feat/f16-s12")?;
    let tok = h.pair_client("f16-t2-4", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f16-8", "F16 S12", h.workspace_dir()).await?;
    let (card_id, _, _) =
        start_and_settle_card(&mut ws, "proj-f16-8", &wt, "F16 S12 Card", None).await?;
    fs::write(wt.join("dirty.txt"), "dirty\n")?;
    let card = get_card(&mut ws, &card_id).await?;
    assert_eq!(card["checkReceipts"][0]["status"], "stale");
    let rev_stale = card["revision"].as_i64().ok_or("missing stale rev")?;
    let _ = retry_check(&mut ws, &card_id, rev_stale, "CHK-1").await?;
    let post = get_card(&mut ws, &card_id).await?;
    assert_eq!(post["checkReceipts"][0]["status"], "passed");
    assert_ne!(
        post["checkReceipts"][0]["productSnapshotDigest"],
        card["checkReceipts"][0]["productSnapshotDigest"]
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f16_t2_5_missing_executable_in_planned_check_records_structured_failure() -> TestResult<()>
{
    let h = CrashableHarness::start(true)?;
    assert!(
        require_m3_execute_ship(
            &h,
            "f16_t2_5_missing_executable_in_planned_check_records_structured_failure",
        )
        .await?,
        "required workflow board capability"
    );
    let wt = h.temp_root().join("wt-f16-nobin");
    create_worktree(h.workspace_dir(), &wt, "feat/f16-nobin")?;
    let tok = h.pair_client("f16-t2-5", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f16-9", "F16 NoBin", h.workspace_dir()).await?;
    let (card_id, rev) = setup_approved_plan_card(
        &mut ws,
        "proj-f16-9",
        &wt,
        "F16 NoBin Card",
        Some(vec!["nonexistent-legion-check-bin-xyz"]),
    )
    .await?;
    let error = expect_rpc_err_tag(
        &mut ws,
        "workflowBoard.retryStep",
        json!({
            "cardId": card_id, "expectedRevision": rev, "stepKind": "check", "targetId": "CHK-1",
        }),
        "WorkflowBoardRequestError",
    )
    .await?;
    assert_eq!(error["code"], "launch_failed");
    let card = get_card(&mut ws, &card_id).await?;
    let receipt = &card["checkReceipts"][0];
    assert_eq!(receipt["status"], "failed");
    assert_eq!(receipt.get("exitCode"), Some(&serde_json::Value::Null));
    assert!(
        receipt["productSnapshotDigest"]
            .as_str()
            .is_some_and(|digest| digest.starts_with("sha256:"))
    );
    assert!(
        receipt["stderrExcerpt"]
            .as_str()
            .is_some_and(|error| error.contains("nonexistent-legion-check-bin-xyz"))
    );
    assert!(receipt["logRef"].is_string());
    Ok(())
}
