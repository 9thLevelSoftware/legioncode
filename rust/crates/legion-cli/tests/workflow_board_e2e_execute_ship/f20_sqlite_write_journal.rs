//! F20: SQLite card service, `.legion/workflows/<card-id>/` & write journal (5 Tier 1 + 5 Tier 2).

use std::fs;

use serde_json::json;
use sha2::{Digest, Sha256};

use super::support::{CrashableHarness, next_id};
use super::support_board::{
    assert_m3_execute_ship, create_project, expect_rpc_err_any_tag, expect_rpc_err_tag, get_card,
    require_board, require_m3_execute_ship, setup_approved_plan_card, start_stage_settled,
};
use crate::workflow_board_e2e::TestResult;
use crate::workflow_board_e2e::git_fixtures::create_worktree;
use crate::workflow_board_e2e::ws_rpc::OpaqueWsClient;
use legion_protocol::rpc_codec::{RpcCauseReason, RpcExit};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f20_t1_1_sqlite_migration_head_and_ws_auth_origin_guards() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    let db = h.open_db_readonly()?;
    let head: i64 = db.query_row(
        "SELECT MAX(migration_id) FROM effect_sql_migrations",
        [],
        |r| r.get(0),
    )?;
    assert!(head >= 110);

    let addr = h.server()?.addr();
    let Err(bad_origin) = OpaqueWsClient::connect(
        addr,
        "/ws?orchestrationProtocol=2",
        &[("Origin", "https://evil.example")],
    )
    .await
    else {
        return Err("untrusted origin must be rejected".into());
    };
    assert!(bad_origin.to_string().contains("403"));

    let Err(proxy_unauth) = OpaqueWsClient::connect(
        addr,
        "/ws?orchestrationProtocol=2",
        &[("x-forwarded-for", "203.0.113.9")],
    )
    .await
    else {
        return Err("non-loopback unauthenticated WS must be rejected".into());
    };
    assert!(proxy_unauth.to_string().contains("401"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f20_t1_2_workspace_deliverables_under_dot_legion_and_host_only_receipts() -> TestResult<()>
{
    let h = CrashableHarness::start(true)?;
    assert_m3_execute_ship(
        &h,
        "f20_t1_2_workspace_deliverables_under_dot_legion_and_host_only_receipts",
    )
    .await?;
    let wt = h.temp_root().join("wt-f20-files");
    create_worktree(h.workspace_dir(), &wt, "feat/f20-files")?;
    let tok = h.pair_client("f20-t1-2", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f20-2", "F20 Files", h.workspace_dir()).await?;
    let (card_id, _) =
        setup_approved_plan_card(&mut ws, "proj-f20-2", &wt, "F20 Files Card", None).await?;

    let card_dir = wt.join(".legion").join("workflows").join(&card_id);
    assert!(card_dir.join("spec.md").is_file());
    assert!(card_dir.join("plan.md").is_file());
    assert!(!card_dir.join("receipts.sqlite").exists());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f20_t1_3_card_revisions_approvals_and_links_survive_restart() -> TestResult<()> {
    let mut h = CrashableHarness::start(true)?;
    assert_m3_execute_ship(
        &h,
        "f20_t1_3_card_revisions_approvals_and_links_survive_restart",
    )
    .await?;
    let wt = h.temp_root().join("wt-f20-rst");
    create_worktree(h.workspace_dir(), &wt, "feat/f20-rst")?;
    let tok = h.pair_client("f20-t1-3", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f20-3", "F20 Rst", h.workspace_dir()).await?;
    let (card_id, rev) =
        setup_approved_plan_card(&mut ws, "proj-f20-3", &wt, "F20 Rst Card", None).await?;
    drop(ws);

    h.restart_server()?;
    let tok2 = h.pair_client("f20-t1-3b", None).await?;
    let mut ws2 = h.connect_ws(&tok2).await?;
    let card = get_card(&mut ws2, &card_id).await?;
    assert_eq!(card["revision"], rev);
    assert_eq!(card["specApproval"]["status"], "valid");
    assert!(
        card["planApprovalStatus"] == "valid"
            || card["planApproval"]["status"] == "valid"
            || card["planApproved"] == true
    );
    assert!(card["revisions"].as_array().is_some_and(|r| !r.is_empty()));
    assert!(card["stageRuns"].is_array());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f20_t1_4_write_journal_crash_recovery_reconciles_on_restart() -> TestResult<()> {
    let mut h = CrashableHarness::start(true)?;
    assert_m3_execute_ship(
        &h,
        "f20_t1_4_write_journal_crash_recovery_reconciles_on_restart",
    )
    .await?;
    let wt = h.temp_root().join("wt-f20-journal");
    create_worktree(h.workspace_dir(), &wt, "feat/f20-journal")?;
    let tok = h.pair_client("f20-t1-4", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f20-4", "F20 Journal", h.workspace_dir()).await?;
    let (card_id, _) =
        setup_approved_plan_card(&mut ws, "proj-f20-4", &wt, "F20 Journal Card", None).await?;
    let spec_path = wt.join(format!(".legion/workflows/{card_id}/spec.md"));
    let expected_digest = format!("sha256:{:x}", Sha256::digest(fs::read(&spec_path)?));
    drop(ws);

    h.stop_server();
    fs::write(
        spec_path.with_extension("md.tmp"),
        "uncommitted-torn-write\n",
    )?;
    h.restart_server()?;
    let tok2 = h.pair_client("f20-t1-4b", None).await?;
    let mut ws2 = h.connect_ws(&tok2).await?;
    let card = get_card(&mut ws2, &card_id).await?;
    assert_eq!(card["cardId"], card_id);
    assert_eq!(card["specApproval"]["status"], "valid");
    assert_eq!(card["specApproval"]["specDigest"], expected_digest);
    let art = ws2
        .call_ok(
            &next_id("art"),
            "workflowBoard.getArtifact",
            json!({ "cardId": card_id, "key": "spec.md" }),
        )
        .await?;
    assert!(
        art["content"]
            .as_str()
            .is_some_and(|c| !c.contains("uncommitted-torn-write"))
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f20_t1_5_external_edit_reconciliation_invalidates_stale_approvals() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    assert_m3_execute_ship(
        &h,
        "f20_t1_5_external_edit_reconciliation_invalidates_stale_approvals",
    )
    .await?;
    let wt = h.temp_root().join("wt-f20-ext");
    create_worktree(h.workspace_dir(), &wt, "feat/f20-ext")?;
    let tok = h.pair_client("f20-t1-5", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f20-5", "F20 Ext", h.workspace_dir()).await?;
    let (card_id, rev) =
        setup_approved_plan_card(&mut ws, "proj-f20-5", &wt, "F20 Ext Card", None).await?;

    let spec_path = wt.join(format!(".legion/workflows/{card_id}/spec.md"));
    fs::write(&spec_path, "# Edited externally on disk\n")?;
    let _ = expect_rpc_err_tag(
        &mut ws,
        "workflowBoard.startStage",
        json!({ "cardId": card_id, "expectedRevision": rev, "stage": "execute" }),
        "WorkflowBoardRequestError",
    )
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f20_t2_1_missing_card_returns_typed_error_cleanly() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    let env_desc = h.probe_environment().await?;
    let tok = h.pair_client("f20-t2-1", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    if env_desc["capabilities"]["projectWorkflowBoard"].as_bool() == Some(true) {
        let err = expect_rpc_err_any_tag(
            &mut ws,
            "workflowBoard.getCard",
            json!({ "cardId": "missing-card-f20" }),
            &["WorkflowBoardRequestError"],
        )
        .await?;
        assert_eq!(err["code"], "card_not_found");
    } else {
        let _ = expect_rpc_err_tag(
            &mut ws,
            "workflowBoard.getCard",
            json!({ "cardId": "missing-card-f20" }),
            "UnsupportedRpcMethod",
        )
        .await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f20_t2_2_concurrent_expected_revision_updates_serialize_without_lost_update()
-> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    assert!(
        require_m3_execute_ship(
            &h,
            "f20_t2_2_concurrent_expected_revision_updates_serialize_without_lost_update",
        )
        .await?,
        "required workflow board capability"
    );
    let wt = h.temp_root().join("wt-f20-conc");
    create_worktree(h.workspace_dir(), &wt, "feat/f20-conc")?;
    let tok1 = h.pair_client("f20-t2-2a", None).await?;
    let tok2 = h.pair_client("f20-t2-2b", None).await?;
    let mut ws1 = h.connect_ws(&tok1).await?;
    let mut ws2 = h.connect_ws(&tok2).await?;
    create_project(&mut ws1, "proj-f20-6", "F20 Conc", h.workspace_dir()).await?;
    let (card_id, rev) =
        setup_approved_plan_card(&mut ws1, "proj-f20-6", &wt, "F20 Conc Card", None).await?;

    let valid_plan = json!({
        "tasks": [{ "id": "TSK-1", "title": "Updated task", "dependsOn": [], "filesAllowed": ["src/ok.ts"], "filesForbidden": [] }],
        "checks": [{ "id": "CHK-1", "criterionId": "AC-1", "argv": ["git", "status", "--porcelain"] }],
        "criterionMappings": [{ "criterionId": "AC-1", "method": "automated", "checkIds": ["CHK-1"] }, { "criterionId": "AC-2", "method": "manual" }],
    });
    let (id1, id2) = (next_id("up1"), next_id("up2"));
    let (r1, r2) = tokio::join!(
        ws1.call_unary(
            &id1,
            "workflowBoard.updatePlan",
            json!({ "cardId": card_id, "expectedRevision": rev, "plan": valid_plan })
        ),
        ws2.call_unary(
            &id2,
            "workflowBoard.updateSpec",
            json!({ "cardId": card_id, "expectedRevision": rev, "brief": "race" }),
        ),
    );
    let (r1, r2) = (r1?, r2?);
    assert_eq!(
        u8::from(matches!(r1, RpcExit::Success { .. }))
            + u8::from(matches!(r2, RpcExit::Success { .. })),
        1
    );
    let ((RpcExit::Failure { cause }, _) | (_, RpcExit::Failure { cause })) = (r1, r2) else {
        return Err("expected exactly one concurrent revision failure".into());
    };
    let tag = match cause.first() {
        Some(RpcCauseReason::Fail { error }) => error["_tag"].as_str(),
        _ => None,
    };
    assert_eq!(tag, Some("WorkflowBoardRequestError"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f20_t2_3_deleted_artifact_during_crash_surfaces_conflict_instead_of_forging_approval()
-> TestResult<()> {
    let mut h = CrashableHarness::start(true)?;
    assert!(
        require_m3_execute_ship(
            &h,
            "f20_t2_3_deleted_artifact_during_crash_surfaces_conflict_instead_of_forging_approval",
        )
        .await?,
        "required workflow board capability"
    );
    let wt = h.temp_root().join("wt-f20-del");
    create_worktree(h.workspace_dir(), &wt, "feat/f20-del")?;
    let tok = h.pair_client("f20-t2-3", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f20-7", "F20 Del", h.workspace_dir()).await?;
    let (card_id, rev) =
        setup_approved_plan_card(&mut ws, "proj-f20-7", &wt, "F20 Del Card", None).await?;
    drop(ws);

    h.stop_server();
    let spec_path = wt.join(format!(".legion/workflows/{card_id}/spec.md"));
    fs::remove_file(&spec_path)?;
    h.restart_server()?;
    let tok2 = h.pair_client("f20-t2-3b", None).await?;
    let mut ws2 = h.connect_ws(&tok2).await?;
    let _ = expect_rpc_err_tag(
        &mut ws2,
        "workflowBoard.startStage",
        json!({ "cardId": card_id, "expectedRevision": rev, "stage": "execute" }),
        "WorkflowBoardRequestError",
    )
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f20_t2_4_path_traversal_in_artifact_request_rejected() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    assert!(
        require_board(&h, "f20_t2_4_path_traversal_in_artifact_request_rejected").await?,
        "required workflow board capability"
    );
    let tok = h.pair_client("f20-t2-4", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    let _ = expect_rpc_err_any_tag(
        &mut ws,
        "workflowBoard.getArtifact",
        json!({ "cardId": "../../etc", "key": "../passwd" }),
        &["ParseError", "WorkflowBoardRequestError"],
    )
    .await?;
    let _ = expect_rpc_err_any_tag(
        &mut ws,
        "workflowBoard.getArtifact",
        json!({ "cardId": "card-20", "key": "../passwd" }),
        &["ParseError", "WorkflowBoardRequestError"],
    )
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f20_t2_5_distinct_card_and_run_lifetimes_survive_run_pruning() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    assert!(
        require_m3_execute_ship(
            &h,
            "f20_t2_5_distinct_card_and_run_lifetimes_survive_run_pruning",
        )
        .await?,
        "required workflow board capability"
    );
    let wt = h.temp_root().join("wt-f20-life");
    create_worktree(h.workspace_dir(), &wt, "feat/f20-life")?;
    let tok = h.pair_client("f20-t2-5", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f20-8", "F20 Life", h.workspace_dir()).await?;
    let (card_id, rev) =
        setup_approved_plan_card(&mut ws, "proj-f20-8", &wt, "F20 Life Card", None).await?;
    let r1 = start_stage_settled(&mut ws, &card_id, rev, "execute", None).await?;
    let r2 = ws
        .call_ok(
            &next_id("s2"),
            "workflowBoard.startStage",
            json!({ "cardId": card_id, "expectedRevision": r1["revision"], "stage": "review" }),
        )
        .await?;
    let run_2 = r2["runId"].as_str().ok_or("missing runId")?;
    let _ = ws
        .call_ok(
            &next_id("can"),
            "workflows.cancel",
            json!({ "runId": run_2, "reason": "cancel run lifecycle" }),
        )
        .await?;
    let card = get_card(&mut ws, &card_id).await?;
    assert_eq!(card["cardId"], card_id);
    assert_eq!(card["specApprovalStatus"], "valid");
    assert!(
        card["planApprovalStatus"] == "valid"
            || card["planApproval"]["status"] == "valid"
            || card["planApproved"] == true
    );
    assert!(
        wt.join(".legion")
            .join("workflows")
            .join(&card_id)
            .join("spec.md")
            .is_file()
    );
    Ok(())
}
