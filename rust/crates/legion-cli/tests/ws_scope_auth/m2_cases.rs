use super::helpers::{
    TestResult, assert_auth_denied, call_unary, connect_ws, issue_token, pair_standard_client,
    spawn_srv,
};
use legion_protocol::rpc_codec::{RpcCauseReason, RpcExit};
use serde_json::json;

#[tokio::test]
async fn ws_enforces_m2_client_only_and_scope_guards() -> TestResult {
    let temp = tempfile::tempdir()?;
    let server = spawn_srv(&temp.path().join("m2-ws-scope")).await?;

    let client_tok = pair_standard_client(&server).await?;
    let read_sess = issue_token(&server, "thread:thr-read-only", &["orchestration:read"])?;
    let thread_op = issue_token(&server, "thread:thr-agent-1", &["orchestration:operate"])?;
    let wildcard_agent = issue_token(&server, "thread:thr-agent-wild", &["*"])?;
    let mcp_oauth = issue_token(&server, "mcp-oauth:sess-1", &["*"])?;
    let peer_link = issue_token(&server, "peer-link:peer-1", &["*"])?;

    for tok in [&thread_op, &wildcard_agent, &mcp_oauth, &peer_link] {
        let mut ws = connect_ws(&server, tok).await?;
        for method in [
            "workflowBoard.approvePlan",
            "workflowBoard.returnForRevision",
        ] {
            let RpcExit::Failure { cause } = call_unary(
                &mut ws,
                "m2-cg",
                method,
                json!({ "cardId": "card-1", "expectedRevision": 1 }),
            )
            .await?
            else {
                panic!("expected client-only denial for {method}");
            };
            let Some(RpcCauseReason::Fail { error }) = cause.first() else {
                panic!("expected Fail cause, got {cause:?}");
            };
            assert_eq!(error["_tag"], "EnvironmentAuthorizationError");
        }
    }

    let mut ws_read = connect_ws(&server, &read_sess).await?;
    for method in [
        "workflowBoard.exploreStep",
        "workflowBoard.challengeStep",
        "workflowBoard.updatePlan",
    ] {
        let exit = call_unary(
            &mut ws_read,
            "m2-read",
            method,
            json!({ "cardId": "card-1", "expectedRevision": 1 }),
        )
        .await?;
        assert_auth_denied(exit, "orchestration:operate", "orchestration:operate");
    }

    let mut ws_op = connect_ws(&server, &thread_op).await?;
    for (method, payload) in [
        (
            "workflowBoard.exploreStep",
            json!({ "cardId": "card-missing", "expectedRevision": 1, "action": "step" }),
        ),
        (
            "workflowBoard.challengeStep",
            json!({ "cardId": "card-missing", "expectedRevision": 1, "action": "start" }),
        ),
        (
            "workflowBoard.updatePlan",
            json!({ "cardId": "card-missing", "expectedRevision": 1 }),
        ),
    ] {
        let RpcExit::Failure { cause } = call_unary(&mut ws_op, "m2-op", method, payload).await?
        else {
            panic!("expected card_not_found for {method}");
        };
        let Some(RpcCauseReason::Fail { error }) = cause.first() else {
            panic!("expected Fail cause, got {cause:?}");
        };
        assert_eq!(error["_tag"], "WorkflowBoardRequestError");
        assert_eq!(error["code"], "card_not_found");
    }

    let mut ws_client = connect_ws(&server, &client_tok).await?;
    for (method, payload) in [
        (
            "workflowBoard.approvePlan",
            json!({ "cardId": "card-missing", "expectedRevision": 1 }),
        ),
        (
            "workflowBoard.returnForRevision",
            json!({ "cardId": "card-missing", "expectedRevision": 1, "targetStage": "spec", "reason": "Revise" }),
        ),
    ] {
        let RpcExit::Failure { cause } =
            call_unary(&mut ws_client, "m2-cl", method, payload).await?
        else {
            panic!("expected card_not_found for {method}");
        };
        let Some(RpcCauseReason::Fail { error }) = cause.first() else {
            panic!("expected Fail cause, got {cause:?}");
        };
        assert_eq!(error["_tag"], "WorkflowBoardRequestError");
        assert_eq!(error["code"], "card_not_found");
    }

    server.shutdown().await?;
    Ok(())
}
