//! Unconditional baseline + F1-F3 board navigation Rust E2E tests.

use serde_json::json;

use crate::workflow_board_e2e::TestResult;

use super::helpers::{
    PlanningEnv, assert_board_capable, call_expect_failure, check_board_capable,
    create_and_approve_spec, populate_and_approve_plan,
};

pub async fn uncond_r01_unsupported_rpc_preserves_ws_and_workflows_subscribe() -> TestResult<()> {
    let mut env = PlanningEnv::start(true).await?;
    if !env.board_supported {
        let err1 = call_expect_failure(
            &mut env.ws,
            "u-get-card",
            "workflowBoard.getCard",
            json!({ "cardId": "card-1" }),
        )
        .await?;
        assert!(err1.to_string().contains("UnsupportedRpcMethod"));

        let err2 = call_expect_failure(
            &mut env.ws,
            "u-create-card",
            "workflowBoard.createCard",
            json!({ "projectId": env.project_id, "title": "Legacy" }),
        )
        .await?;
        assert!(err2.to_string().contains("UnsupportedRpcMethod"));
    }

    env.ws
        .subscribe("wf-sub", "workflows.subscribe", json!({}))
        .await?;
    let snap = env.ws.next_chunk_for("wf-sub").await?;
    assert!(snap.is_object());
    env.ws.interrupt("wf-sub").await?;

    let probe = env.ws.call_ok("u-probe", "server.probe", json!({})).await?;
    assert!(probe["serverTime"].as_str().is_some());
    Ok(())
}

pub async fn uncond_r02_orchestration_read_token_rejected_on_workflow_board_create_card()
-> TestResult<()> {
    let mut env = PlanningEnv::start(true).await?;
    env.reconnect_ws("e2e-rust-read-only", Some(&["orchestration:read"]))
        .await?;

    let probe = env.ws.call_ok("r-probe", "server.probe", json!({})).await?;
    assert!(probe["serverTime"].as_str().is_some());

    let create_err = call_expect_failure(
        &mut env.ws,
        "r-create",
        "workflowBoard.createCard",
        json!({ "projectId": env.project_id, "title": "Read-only must fail" }),
    )
    .await?;
    assert!(
        create_err
            .to_string()
            .contains("EnvironmentAuthorizationError"),
        "expected EnvironmentAuthorizationError, got {create_err}"
    );

    let approve_err = call_expect_failure(
        &mut env.ws,
        "r-approve",
        "workflowBoard.approveSpec",
        json!({ "cardId": "card-1", "expectedRevision": 1 }),
    )
    .await?;
    assert!(
        approve_err
            .to_string()
            .contains("EnvironmentAuthorizationError"),
        "expected EnvironmentAuthorizationError, got {approve_err}"
    );
    Ok(())
}

pub async fn uncond_r03_scoped_read_snapshot_vs_dispatch_command_guard() -> TestResult<()> {
    let mut env = PlanningEnv::start(true).await?;
    env.reconnect_ws("e2e-rust-read-guard", Some(&["orchestration:read"]))
        .await?;

    let shell = env
        .ws
        .call_ok("r-shell", "orchestration.v2.getShellSnapshot", json!({}))
        .await?;
    assert_eq!(shell["projects"].as_array().map(Vec::len), Some(1));

    let dispatch_err = call_expect_failure(
        &mut env.ws,
        "r-dispatch",
        "orchestration.v2.dispatchCommand",
        json!({
            "type": "project.create",
            "commandId": "cmd-unauth-create",
            "title": "Should fail auth",
            "workspaceRoot": env.harness.workspace_dir().to_string_lossy(),
        }),
    )
    .await?;
    assert!(
        dispatch_err
            .to_string()
            .contains("EnvironmentAuthorizationError"),
        "expected EnvironmentAuthorizationError, got {dispatch_err}"
    );
    Ok(())
}

pub async fn f1_t2_05_board_overview_survives_restart() -> TestResult<()> {
    let mut env = PlanningEnv::start(true).await?;
    assert!(check_board_capable(&env, "F1-T2-05"), "required workflow board capability");

    let created_paused = env
        .ws
        .call_ok(
            "c-f1-paused",
            "workflowBoard.createCard",
            json!({
                "projectId": env.project_id,
                "title": "Spec Paused Card",
                "brief": "Brief for Spec Paused Card",
                "sourceType": "idea",
                "assistanceMode": "guided",
            }),
        )
        .await?;
    let paused_id = created_paused["cardId"]
        .as_str()
        .ok_or("missing paused cardId")?
        .to_owned();
    let paused_rev = created_paused["revision"]
        .as_i64()
        .ok_or("missing paused revision")?;
    env.ws
        .call_ok(
            "e-f1-paused",
            "workflowBoard.exploreStep",
            json!({
                "cardId": paused_id,
                "expectedRevision": paused_rev,
                "answers": [{ "decisionId": "DEC-1", "action": "pause" }],
            }),
        )
        .await?;

    let (plan_stage_id, _rev_plan_stage) =
        create_and_approve_spec(&mut env.ws, &env.project_id, "Plan Stage Card", false).await?;

    let (plan_approved_id, rev_spec_3) =
        create_and_approve_spec(&mut env.ws, &env.project_id, "Plan Approved Card", false).await?;
    let _rev_plan_app =
        populate_and_approve_plan(&mut env.ws, &plan_approved_id, rev_spec_3).await?;

    env.harness.restart_server()?;
    env.reconnect_ws("e2e-f1-post-restart", None).await?;
    env.ws
        .subscribe(
            "sub-f1",
            "workflowBoard.subscribe",
            json!({ "projectId": env.project_id }),
        )
        .await?;
    let snap = env.ws.next_chunk_for("sub-f1").await?;
    let cards = snap["cards"].as_array().ok_or("missing cards array")?;
    assert_eq!(cards.len(), 3);

    let find_card = |id: &str| {
        cards
            .iter()
            .find(|c| c["cardId"].as_str() == Some(id))
            .ok_or_else(|| format!("missing card {id} after restart"))
    };
    let card_paused = find_card(&paused_id)?;
    let card_plan_stage = find_card(&plan_stage_id)?;
    let card_plan_approved = find_card(&plan_approved_id)?;

    assert_eq!(card_paused["title"].as_str(), Some("Spec Paused Card"));
    assert_eq!(card_paused["stage"].as_str(), Some("spec"));
    assert_eq!(card_plan_stage["title"].as_str(), Some("Plan Stage Card"));
    assert_eq!(card_plan_stage["stage"].as_str(), Some("plan"));
    assert_eq!(
        card_plan_approved["title"].as_str(),
        Some("Plan Approved Card")
    );
    assert_ne!(card_plan_approved["stage"].as_str(), Some("spec"));
    assert_ne!(card_plan_approved["stage"].as_str(), Some("plan"));
    env.ws.interrupt("sub-f1").await?;

    let paused_detail = env
        .ws
        .call_ok(
            "get-f1-paused",
            "workflowBoard.getCard",
            json!({ "cardId": paused_id }),
        )
        .await?;
    assert!(
        paused_detail["exploration"]["unresolvedIds"]
            .as_array()
            .is_some_and(|ids| ids.iter().any(|v| v.as_str() == Some("DEC-1")))
            || paused_detail.to_string().contains("pause")
    );
    Ok(())
}

pub async fn f2_t1_05_card_retains_identity_and_history_across_attempts() -> TestResult<()> {
    let mut env = PlanningEnv::start(true).await?;
    assert_board_capable(&env, "F2-T1-05");
    let (card_id, rev1) =
        create_and_approve_spec(&mut env.ws, &env.project_id, "Multi-Attempt", false).await?;
    let rev_plan_app = populate_and_approve_plan(&mut env.ws, &card_id, rev1).await?;

    let returned = env
        .ws
        .call_ok(
            "ret-f2",
            "workflowBoard.returnForRevision",
            json!({
                "cardId": card_id,
                "expectedRevision": rev_plan_app,
                "targetStage": "spec",
                "reason": "Attempt 2 spec refinement",
            }),
        )
        .await?;
    let rev_ret = returned["revision"].as_i64().ok_or("missing rev_ret")?;

    let spec2 = env
        .ws
        .call_ok(
            "u-f2-s2",
            "workflowBoard.updateSpec",
            json!({
                "cardId": card_id,
                "expectedRevision": rev_ret,
                "brief": "Attempt 2 refined brief.",
                "acceptanceCriteria": [
                    { "id": "AC-01", "text": "Domain validates cleanly." },
                    { "id": "AC-02", "text": "State survives restart." }
                ],
                "constraints": ["Zero auto-commits"],
                "sourceRefs": ["README.md"],
            }),
        )
        .await?;
    let rev_after_spec = spec2["revision"].as_i64().ok_or("missing rev_after_spec")?;
    let chal2 = env
        .ws
        .call_ok(
            "c-f2-s2",
            "workflowBoard.challengeStep",
            json!({
                "cardId": card_id,
                "expectedRevision": rev_after_spec,
                "action": "complete",
                "concerns": [],
            }),
        )
        .await?;
    let rev_after_challenge = chal2["revision"]
        .as_i64()
        .ok_or("missing rev_after_challenge")?;
    let app2 = env
        .ws
        .call_ok(
            "a-f2-s2",
            "workflowBoard.approveSpec",
            json!({ "cardId": card_id, "expectedRevision": rev_after_challenge }),
        )
        .await?;
    let rev_spec_2 = app2["revision"].as_i64().ok_or("missing rev_spec_2")?;

    env.harness.restart_server()?;
    env.reconnect_ws("e2e-f2-post-restart", None).await?;
    let detail = env
        .ws
        .call_ok(
            "get-f2",
            "workflowBoard.getCard",
            json!({ "cardId": card_id }),
        )
        .await?;
    assert_eq!(detail["cardId"].as_str(), Some(card_id.as_str()));
    assert!(detail["revision"].as_i64().is_some_and(|r| r >= rev_spec_2));
    assert!(
        detail["history"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );
    assert_eq!(detail["specApproval"]["status"].as_str(), Some("valid"));
    assert_eq!(detail["planApproval"]["status"].as_str(), Some("stale"));
    Ok(())
}

pub async fn f2_t2_05_large_artifact_chunked_over_frame_parts() -> TestResult<()> {
    let mut env = PlanningEnv::start(true).await?;
    assert!(check_board_capable(&env, "F2-T2-05"), "required workflow board capability");
    let huge_body = "Line of large specification artifact for frameParts test.\n".repeat(2800);
    assert!(huge_body.len() > 128 * 1024);

    let created = env
        .ws
        .call_ok(
            "c-huge",
            "workflowBoard.createCard",
            json!({
                "projectId": env.project_id,
                "title": "Large Spec Card",
                "brief": huge_body,
                "sourceType": "idea",
                "assistanceMode": "direct",
            }),
        )
        .await?;
    let card_id = created["cardId"].as_str().ok_or("missing cardId")?;
    let artifact = env
        .ws
        .call_ok(
            "get-huge",
            "workflowBoard.getArtifact",
            json!({ "cardId": card_id, "key": "spec.md" }),
        )
        .await?;
    let content = artifact["content"].as_str().ok_or("missing content")?;
    assert!(content.len() >= huge_body.len() && content.len() > 128 * 1024);
    assert!(
        content.contains(&huge_body),
        "reassembled spec.md must contain full payload"
    );
    Ok(())
}

pub async fn f3_t1_05_revision_return_history_survives_restart() -> TestResult<()> {
    let mut env = PlanningEnv::start(true).await?;
    assert_board_capable(&env, "F3-T1-05");
    let (card_id, rev) =
        create_and_approve_spec(&mut env.ws, &env.project_id, "Return Restart", false).await?;
    let _returned = env
        .ws
        .call_ok(
            "ret-f3",
            "workflowBoard.returnForRevision",
            json!({
                "cardId": card_id,
                "expectedRevision": rev,
                "targetStage": "spec",
                "reason": "Re-verify AC-01 boundary",
            }),
        )
        .await?;

    env.harness.restart_server()?;
    env.reconnect_ws("e2e-f3-post-restart", None).await?;
    let detail = env
        .ws
        .call_ok(
            "get-f3",
            "workflowBoard.getCard",
            json!({ "cardId": card_id }),
        )
        .await?;
    assert_eq!(detail["stage"].as_str(), Some("spec"));
    assert!(
        detail["history"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );
    Ok(())
}
