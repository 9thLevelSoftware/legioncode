//! F22 Tier 1 opaque-box Rust integration tests (`[F22-T1-04]`..`[F22-T1-06]`).

use serde_json::{Value, json};

use super::helpers::{
    assert_auth_denied, assert_not_auth_denied, board_capability_enabled, call_mcp_tool,
    ensure_project_added, extract_card_id, extract_thread_id, mint_scoped_session,
};
use super::workflow_board_e2e::TestResult;
use super::workflow_board_e2e::harness::OpaqueBoxHarness;
use super::workflow_board_e2e::ws_rpc::OpaqueWsClient;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f22_t1_04_ws_scope_enforcement_read_vs_operate() -> TestResult<()> {
    let harness = OpaqueBoxHarness::start(true)?;

    let diag_tok = harness
        .pair_client("diag-only", Some(&["diagnostics:read"]))
        .await?;
    let diag_auth = format!("Bearer {diag_tok}");
    let mut diag_ws = OpaqueWsClient::connect(
        harness.addr()?,
        "/ws?orchestrationProtocol=2",
        &[("Authorization", &diag_auth)],
    )
    .await?;

    for (idx, method) in ["workflowBoard.getCard", "workflowBoard.subscribe"]
        .into_iter()
        .enumerate()
    {
        let exit = diag_ws
            .call_unary(&format!("diag-{idx}"), method, json!({ "cardId": "c1" }))
            .await?;
        assert_auth_denied(&exit, "orchestration:read", "orchestration:read", method)?;
    }
    drop(diag_ws);

    let read_tok = harness
        .pair_client("orch-read", Some(&["orchestration:read"]))
        .await?;
    let read_auth = format!("Bearer {read_tok}");
    let mut read_ws = OpaqueWsClient::connect(
        harness.addr()?,
        "/ws?orchestrationProtocol=2",
        &[("Authorization", &read_auth)],
    )
    .await?;

    let probe = read_ws
        .call_ok("probe-1", "server.probe", json!({}))
        .await?;
    assert!(probe["serverTime"].as_str().is_some());

    for (idx, method) in [
        "workflowBoard.createCard",
        "workflowBoard.updateSpec",
        "workflowBoard.approveSpec",
    ]
    .into_iter()
    .enumerate()
    {
        let exit = read_ws
            .call_unary(&format!("write-{idx}"), method, json!({ "cardId": "c1" }))
            .await?;
        assert_auth_denied(
            &exit,
            "orchestration:operate",
            "orchestration:operate",
            method,
        )?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f22_t1_05_human_only_actions_reject_agent_tokens_and_allow_human_clients() -> TestResult<()>
{
    let harness = OpaqueBoxHarness::start(true)?;
    let admin_tok = harness.pair_client("human-admin", None).await?;

    let thread_tok = mint_scoped_session(
        &harness,
        &admin_tok,
        "thread:thr-e2e-agent",
        &["orchestration:read", "orchestration:operate"],
    )
    .await?;
    let oauth_tok = mint_scoped_session(
        &harness,
        &admin_tok,
        "mcp-oauth",
        &["orchestration:read", "orchestration:operate"],
    )
    .await?;

    let human_only_methods = [
        "workflowBoard.approveSpec",
        "workflowBoard.approvePlan",
        "workflowBoard.approveDelivery",
        "workflowBoard.returnForRevision",
        "workflowBoard.resolveFinding",
        "workflowBoard.recordManualAcceptance",
    ];

    for (label, tok) in [("thread-agent", thread_tok), ("mcp-oauth", oauth_tok)] {
        let auth = format!("Bearer {tok}");
        let mut ws = OpaqueWsClient::connect(
            harness.addr()?,
            "/ws?orchestrationProtocol=2",
            &[("Authorization", &auth)],
        )
        .await?;
        for (idx, method) in human_only_methods.iter().enumerate() {
            let exit = ws
                .call_unary(
                    &format!("{label}-{idx}"),
                    method,
                    json!({ "cardId": "card-1", "expectedRevision": 1 }),
                )
                .await?;
            assert_auth_denied(
                &exit,
                "orchestration:operate",
                "client:interactive",
                &format!("{label}::{method}"),
            )?;
        }
    }

    let admin_auth = format!("Bearer {admin_tok}");
    let mut admin_ws = OpaqueWsClient::connect(
        harness.addr()?,
        "/ws?orchestrationProtocol=2",
        &[("Authorization", &admin_auth)],
    )
    .await?;
    let human_exit = admin_ws
        .call_unary(
            "human-approve",
            "workflowBoard.approveSpec",
            json!({ "cardId": "nonexistent", "expectedRevision": 1 }),
        )
        .await?;
    assert_not_auth_denied(&human_exit, "human-admin::approveSpec")?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f22_t1_06_scoped_agent_mcp_tools_read_brief_and_propose_without_auto_approval()
-> TestResult<()> {
    let harness = OpaqueBoxHarness::start(true)?;
    assert!(
        board_capability_enabled(&harness).await?,
        "[F22-T1-06] capabilities.projectWorkflowBoard must be true"
    );

    let admin_tok = harness.pair_client("mcp-admin", None).await?;
    let admin_auth = format!("Bearer {admin_tok}");
    let mut ws = OpaqueWsClient::connect(
        harness.addr()?,
        "/ws?orchestrationProtocol=2",
        &[("Authorization", &admin_auth)],
    )
    .await?;
    let project_id = ensure_project_added(&harness, &mut ws).await?;
    let created = ws
        .call_ok(
            "create-card",
            "workflowBoard.createCard",
            json!({ "projectId": project_id, "title": "MCP brief card" }),
        )
        .await?;
    let card_id = extract_card_id(&created)?;
    let started = ws
        .call_ok(
            "start-stage",
            "workflowBoard.startStage",
            json!({ "cardId": card_id, "stage": "spec", "expectedRevision": 1 }),
        )
        .await?;
    let thread_id = extract_thread_id(&started)?;
    let thread_tok = mint_scoped_session(
        &harness,
        &admin_tok,
        &format!("thread:{thread_id}"),
        &["orchestration:read", "orchestration:operate"],
    )
    .await?;

    let list_resp = harness
        .post_mcp(
            Some(&thread_tok),
            &[("x-legioncode-thread-id", &thread_id)],
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {} }),
        )
        .await?;
    assert_eq!(list_resp.status().as_u16(), 200);
    let list_json: Value = list_resp.json().await?;
    let tool_names: Vec<&str> = list_json["result"]["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| t.get("name").and_then(Value::as_str))
        .collect();
    assert!(tool_names.contains(&"legion_workflow_board_read_brief"));
    assert!(tool_names.contains(&"legion_workflow_board_propose"));
    assert!(!tool_names.contains(&"legion_workflow"));

    let read_json = call_mcp_tool(
        &harness,
        &thread_tok,
        &thread_id,
        2,
        "legion_workflow_board_read_brief",
        json!({ "cardId": card_id }),
    )
    .await?;
    assert_eq!(read_json["result"]["isError"], false);

    let propose_json = call_mcp_tool(
        &harness,
        &thread_tok,
        &thread_id,
        3,
        "legion_workflow_board_propose",
        json!({
            "cardId": card_id,
            "stage": "spec",
            "markdown": "# Proposed Spec\n\n- AC-1: Agent proposed spec."
        }),
    )
    .await?;
    assert_eq!(propose_json["result"]["isError"], false);

    let detail = ws
        .call_ok(
            "get-after",
            "workflowBoard.getCard",
            json!({ "cardId": card_id }),
        )
        .await?;
    assert_ne!(
        detail["specApprovalStatus"],
        Value::String("valid".to_owned())
    );
    assert!(detail["specApproval"].is_null());
    assert!(detail["specApprovedRevision"].is_null());

    Ok(())
}
