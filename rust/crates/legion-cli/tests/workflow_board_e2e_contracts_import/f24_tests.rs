//! F24 opaque-box Rust integration tests (`[F24-T1-01]`..`[F24-T2-03]`).

use std::fs;

use legion_protocol::rpc_codec::RpcExit;
use serde_json::{Value, json};

use super::helpers::{
    assert_domain_failure, board_capability_enabled, count_files_under, ensure_project_added,
    extract_card_id,
};
use super::workflow_board_e2e::TestResult;
use super::workflow_board_e2e::git_fixtures::{
    CliLockKind, CopyLegionCliOpts, copy_disposable_legion_cli, seed_active_cli_lock,
};
use super::workflow_board_e2e::harness::OpaqueBoxHarness;
use super::workflow_board_e2e::ws_rpc::OpaqueWsClient;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f24_t1_01_preview_real_legion_cli_copy_surfaces_spec_tasks_decisions_blockers_and_policies()
-> TestResult<()> {
    let harness = OpaqueBoxHarness::start(true)?;
    let cli_copy = copy_disposable_legion_cli(
        harness.workspace_dir(),
        CopyLegionCliOpts {
            include_map: false,
            include_cache: false,
        },
    )?;
    assert!(cli_copy.join("workflow").join("execution.yaml").is_file());
    assert!(cli_copy.join("tasks").join("TSK-0002.md").is_file());

    assert!(
        board_capability_enabled(&harness).await?,
        "[F24-T1-01] capabilities.projectWorkflowBoard must be true"
    );

    let token = harness.pair_client("f24-preview", None).await?;
    let auth = format!("Bearer {token}");
    let mut ws = OpaqueWsClient::connect(
        harness.addr()?,
        "/ws?orchestrationProtocol=2",
        &[("Authorization", &auth)],
    )
    .await?;
    let project_id = ensure_project_added(&harness, &mut ws).await?;

    let preview = ws
        .call_ok(
            "preview-1",
            "workflowBoard.previewCliImport",
            json!({ "projectId": project_id }),
        )
        .await?;
    let preview_str = preview.to_string();
    assert!(preview_str.contains("spec-t3-code"));
    for n in 1..=8 {
        assert!(preview_str.contains(&format!("TSK-000{n}")));
        assert!(preview_str.contains(&format!("D-00{n}")));
    }
    assert!(preview_str.contains("blocked"));
    assert!(preview_str.contains("packages/provider-mimo/qualification/cli.test.mjs"));
    assert!(preview_str.contains("ASM-0001"));
    assert!(preview_str.contains("ASM-0018"));
    assert!(preview_str.contains("requireHardened"));
    assert!(preview_str.contains("passScore"));
    assert!(preview_str.contains("trustKeys"));

    ws.subscribe(
        "sub-after-preview",
        "workflowBoard.subscribe",
        json!({ "projectId": project_id }),
    )
    .await?;
    let snap = ws.next_chunk_for("sub-after-preview").await?;
    let cards_len = snap["cards"].as_array().map_or(0, Vec::len);
    assert_eq!(cards_len, 0, "previewCliImport must not mutate card DB");
    ws.interrupt("sub-after-preview").await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f24_t1_04_historical_cli_receipts_require_native_revalidation_before_ship()
-> TestResult<()> {
    let harness = OpaqueBoxHarness::start(true)?;
    copy_disposable_legion_cli(harness.workspace_dir(), CopyLegionCliOpts::default())?;

    assert!(
        board_capability_enabled(&harness).await?,
        "[F24-T1-04] capabilities.projectWorkflowBoard must be true"
    );

    let token = harness.pair_client("f24-historical", None).await?;
    let auth = format!("Bearer {token}");
    let mut ws = OpaqueWsClient::connect(
        harness.addr()?,
        "/ws?orchestrationProtocol=2",
        &[("Authorization", &auth)],
    )
    .await?;
    let project_id = ensure_project_added(&harness, &mut ws).await?;

    let applied = ws
        .call_ok(
            "apply-hist",
            "workflowBoard.applyCliImport",
            json!({ "projectId": project_id }),
        )
        .await?;
    let card_id = extract_card_id(&applied)?;
    let detail = ws
        .call_ok(
            "get-hist",
            "workflowBoard.getCard",
            json!({ "cardId": card_id }),
        )
        .await?;
    let detail_str = detail.to_string();
    assert!(
        detail_str.contains("historical") || detail_str.contains("spec-approval.yaml"),
        "expected historical receipt provenance on imported card: {detail_str}"
    );
    let rev = detail["revision"].as_u64().unwrap_or(1);

    let ship_exit = ws
        .call_unary(
            "ship-hist",
            "workflowBoard.approveDelivery",
            json!({ "cardId": card_id, "expectedRevision": rev }),
        )
        .await?;
    assert_domain_failure(
        &ship_exit,
        &[
            "historical",
            "evidence",
            "validation",
            "revalidat",
            "check",
            "review",
            "observation",
            "stage",
            "gate",
        ],
        "historical CLI receipts alone must not allow Ship delivery",
    )?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f24_t1_05_unsupported_adopted_policies_gate_execute_until_acknowledged() -> TestResult<()>
{
    let harness = OpaqueBoxHarness::start(true)?;
    copy_disposable_legion_cli(harness.workspace_dir(), CopyLegionCliOpts::default())?;

    assert!(
        board_capability_enabled(&harness).await?,
        "[F24-T1-05] capabilities.projectWorkflowBoard must be true"
    );

    let token = harness.pair_client("f24-policy", None).await?;
    let auth = format!("Bearer {token}");
    let mut ws = OpaqueWsClient::connect(
        harness.addr()?,
        "/ws?orchestrationProtocol=2",
        &[("Authorization", &auth)],
    )
    .await?;
    let project_id = ensure_project_added(&harness, &mut ws).await?;

    let applied = ws
        .call_ok(
            "apply-pol",
            "workflowBoard.applyCliImport",
            json!({ "projectId": project_id }),
        )
        .await?;
    let card_id = extract_card_id(&applied)?;
    let detail = ws
        .call_ok(
            "get-pol",
            "workflowBoard.getCard",
            json!({ "cardId": card_id }),
        )
        .await?;
    let detail_str = detail.to_string();
    assert!(detail_str.contains("requireHardened"));
    assert!(detail_str.contains("passScore"));
    assert!(detail_str.contains("trustKeys"));
    let rev = detail["revision"].as_u64().unwrap_or(1);

    let execute_exit = ws
        .call_unary(
            "start-exec-unack",
            "workflowBoard.startStage",
            json!({ "cardId": card_id, "stage": "execute", "expectedRevision": rev }),
        )
        .await?;
    assert_domain_failure(
        &execute_exit,
        &[
            "polic",
            "acknowledg",
            "requirehardened",
            "passscore",
            "trustkeys",
            "adopted",
        ],
        "Execute must stay gated while unsupported adopted policies are unacknowledged",
    )?;

    let ack_exit = ws
        .call_unary(
            "start-exec-ack",
            "workflowBoard.startStage",
            json!({
                "cardId": card_id,
                "stage": "execute",
                "expectedRevision": rev,
                "acknowledgeUnsupportedPolicies": true,
                "acknowledgedPolicies": [
                    "sandbox.requireHardened",
                    "qa.passScore",
                    "skills.trustKeys"
                ]
            }),
        )
        .await?;
    match &ack_exit {
        RpcExit::Success { .. } => {
            let after_ack = ws
                .call_ok(
                    "get-pol-ack",
                    "workflowBoard.getCard",
                    json!({ "cardId": card_id }),
                )
                .await?;
            assert!(after_ack["revision"].as_u64().unwrap_or(0) >= rev);
        }
        RpcExit::Failure { .. } => {
            assert_domain_failure(
                &ack_exit,
                &["blocked", "tsk-0002", "polic", "acknowledg"],
                "post-acknowledgement startStage progression",
            )?;
        }
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f24_t2_02_refuses_takeover_when_engine_lock_active_pid_present() -> TestResult<()> {
    let harness = OpaqueBoxHarness::start(true)?;
    copy_disposable_legion_cli(harness.workspace_dir(), CopyLegionCliOpts::default())?;
    let lock_path = seed_active_cli_lock(harness.workspace_dir(), CliLockKind::EngineLock)?;
    assert!(lock_path.is_file());

    assert!(!(!board_capability_enabled(&harness).await?), "required workflow board capability: [F24-T2-02] engine.lock active run refusal");

    let token = harness.pair_client("f24-engine-lock", None).await?;
    let auth = format!("Bearer {token}");
    let mut ws = OpaqueWsClient::connect(
        harness.addr()?,
        "/ws?orchestrationProtocol=2",
        &[("Authorization", &auth)],
    )
    .await?;
    let project_id = ensure_project_added(&harness, &mut ws).await?;

    let preview = ws
        .call_unary(
            "preview-locked",
            "workflowBoard.previewCliImport",
            json!({ "projectId": project_id }),
        )
        .await?;
    match preview {
        RpcExit::Success { value } => {
            let s = value.to_string().to_ascii_lowercase();
            assert!(s.contains("active") || s.contains("lock") || s.contains("conflict"));
        }
        RpcExit::Failure { cause } => {
            let s = format!("{cause:?}").to_ascii_lowercase();
            assert!(s.contains("active") || s.contains("lock"));
        }
    }

    let apply = ws
        .call_unary(
            "apply-locked",
            "workflowBoard.applyCliImport",
            json!({ "projectId": project_id }),
        )
        .await?;
    assert_domain_failure(
        &apply,
        &["active", "lock", "running", "cli", "refus", "engine.lock"],
        "applyCliImport must refuse active engine.lock",
    )?;
    assert_eq!(
        count_files_under(&harness.workspace_dir().join(".legion").join("workflows"))?,
        0
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f24_t2_03_refuses_takeover_for_live_spawn_and_resume_json_active_locks() -> TestResult<()>
{
    let harness = OpaqueBoxHarness::start(true)?;
    copy_disposable_legion_cli(harness.workspace_dir(), CopyLegionCliOpts::default())?;
    let live_lock = seed_active_cli_lock(harness.workspace_dir(), CliLockKind::LiveSpawn)?;
    assert!(live_lock.is_file());

    assert!(!(!board_capability_enabled(&harness).await?), "required workflow board capability: [F24-T2-03] live-spawn & resume.json lock refusal");

    let token = harness.pair_client("f24-live-resume-lock", None).await?;
    let auth = format!("Bearer {token}");
    let mut ws = OpaqueWsClient::connect(
        harness.addr()?,
        "/ws?orchestrationProtocol=2",
        &[("Authorization", &auth)],
    )
    .await?;
    let project_id = ensure_project_added(&harness, &mut ws).await?;

    let live_exit = ws
        .call_unary(
            "apply-live-lock",
            "workflowBoard.applyCliImport",
            json!({ "projectId": project_id }),
        )
        .await?;
    assert_domain_failure(
        &live_exit,
        &["active", "lock", "running", "cli", "refus", "live-spawn"],
        "applyCliImport must refuse active live-spawn lock",
    )?;
    fs::remove_file(&live_lock)?;

    let resume_lock = seed_active_cli_lock(harness.workspace_dir(), CliLockKind::ResumeJson)?;
    assert!(resume_lock.is_file());
    let resume_exit = ws
        .call_unary(
            "apply-resume-lock",
            "workflowBoard.applyCliImport",
            json!({ "projectId": project_id }),
        )
        .await?;
    assert_domain_failure(
        &resume_exit,
        &["active", "lock", "running", "cli", "refus", "resume.json"],
        "applyCliImport must refuse active resume.json lock",
    )?;
    fs::remove_file(&resume_lock)?;

    let recovered: Value = ws
        .call_ok(
            "apply-after-unlock",
            "workflowBoard.applyCliImport",
            json!({ "projectId": project_id }),
        )
        .await?;
    let card_id = extract_card_id(&recovered)?;
    assert_ne!(card_id, "");

    Ok(())
}
