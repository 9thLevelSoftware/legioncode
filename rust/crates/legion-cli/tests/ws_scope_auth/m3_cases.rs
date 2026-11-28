use super::helpers::{
    TestResult, assert_auth_denied, call_unary, connect_ws, issue_token, pair_standard_client,
    spawn_srv,
};
use legion_protocol::rpc_codec::{RpcCauseReason, RpcExit};
use serde_json::json;

#[tokio::test]
async fn ws_enforces_m3_client_only_and_scope_guards() -> TestResult {
    let temp = tempfile::tempdir()?;
    let server = spawn_srv(&temp.path().join("m3-ws-scope")).await?;

    let client_tok = pair_standard_client(&server).await?;
    let read_sess = issue_token(&server, "thread:thr-m3-read", &["orchestration:read"])?;
    let thread_op = issue_token(&server, "thread:thr-m3-op", &["orchestration:operate"])?;
    let wildcard_agent = issue_token(&server, "thread:thr-m3-wild", &["*"])?;
    let mcp_oauth = issue_token(&server, "mcp-oauth:sess-m3", &["*"])?;
    let peer_link = issue_token(&server, "peer-link:peer-m3", &["*"])?;

    for tok in [&thread_op, &wildcard_agent, &mcp_oauth, &peer_link] {
        let mut ws = connect_ws(&server, tok).await?;
        for (method, payload) in [
            (
                "workflowBoard.resolveFinding",
                json!({ "cardId": "card-1", "expectedRevision": 1, "findingId": "fnd-1", "disposition": "fix" }),
            ),
            (
                "workflowBoard.recordManualAcceptance",
                json!({ "cardId": "card-1", "expectedRevision": 1, "criterionId": "AC-1", "verdict": "pass" }),
            ),
            (
                "workflowBoard.approveDelivery",
                json!({ "cardId": "card-1", "expectedRevision": 1 }),
            ),
        ] {
            let RpcExit::Failure { cause } = call_unary(&mut ws, "m3-cg", method, payload).await?
            else {
                panic!("expected client-only denial for {method}");
            };
            let Some(RpcCauseReason::Fail { error }) = cause.first() else {
                panic!("expected Fail cause, got {cause:?}");
            };
            assert_eq!(error["_tag"], "EnvironmentAuthorizationError");
        }
    }

    assert_m3_read_and_client_only_denials(&server, &read_sess).await?;
    assert_m3_allowed_card_not_found(&server, &thread_op, &client_tok).await?;

    server.shutdown().await?;
    Ok(())
}

async fn assert_m3_read_and_client_only_denials(
    server: &legion_cli::serve::RunningServer,
    read_sess: &str,
) -> TestResult {
    let mut ws_read = connect_ws(server, read_sess).await?;
    for (method, payload) in [
        (
            "workflowBoard.pauseCard",
            json!({ "cardId": "card-1", "expectedRevision": 1 }),
        ),
        (
            "workflowBoard.resumeCard",
            json!({ "cardId": "card-1", "expectedRevision": 1 }),
        ),
        (
            "workflowBoard.retryStep",
            json!({ "cardId": "card-1", "expectedRevision": 1, "stepKind": "stage" }),
        ),
        (
            "workflowBoard.proposeAmendment",
            json!({ "cardId": "card-1", "expectedRevision": 1, "reason": "Add step" }),
        ),
    ] {
        let exit = call_unary(&mut ws_read, "m3-read", method, payload).await?;
        assert_auth_denied(exit, "orchestration:operate", "orchestration:operate");
    }

    let client_read = issue_token(server, "client:read-only", &["orchestration:read"])?;
    let mut ws_client_read = connect_ws(server, &client_read).await?;
    for (method, payload) in [
        (
            "workflowBoard.resolveFinding",
            json!({ "cardId": "card-1", "expectedRevision": 1, "findingId": "fnd-1", "disposition": "fix" }),
        ),
        (
            "workflowBoard.recordManualAcceptance",
            json!({ "cardId": "card-1", "expectedRevision": 1, "criterionId": "AC-1", "verdict": "pass" }),
        ),
        (
            "workflowBoard.approveDelivery",
            json!({ "cardId": "card-1", "expectedRevision": 1 }),
        ),
    ] {
        let exit_agent = call_unary(&mut ws_read, "m3-read-cg", method, payload.clone()).await?;
        assert_auth_denied(exit_agent, "orchestration:operate", "client:interactive");
        let exit_client = call_unary(&mut ws_client_read, "m3-cread", method, payload).await?;
        assert_auth_denied(
            exit_client,
            "orchestration:operate",
            "orchestration:operate",
        );
    }

    let RpcExit::Failure { cause } = call_unary(
        &mut ws_read,
        "m3-diff-read",
        "workflowBoard.getCardDiff",
        json!({ "cardId": "card-missing" }),
    )
    .await?
    else {
        panic!("expected card_not_found for getCardDiff");
    };
    let Some(RpcCauseReason::Fail { error }) = cause.first() else {
        panic!("expected Fail cause, got {cause:?}");
    };
    assert_eq!(error["_tag"], "WorkflowBoardRequestError");
    assert_eq!(error["code"], "card_not_found");
    Ok(())
}

async fn assert_m3_allowed_card_not_found(
    server: &legion_cli::serve::RunningServer,
    thread_op: &str,
    client_tok: &str,
) -> TestResult {
    let mut ws_op = connect_ws(server, thread_op).await?;
    for (method, payload) in [
        (
            "workflowBoard.pauseCard",
            json!({ "cardId": "card-missing", "expectedRevision": 1 }),
        ),
        (
            "workflowBoard.resumeCard",
            json!({ "cardId": "card-missing", "expectedRevision": 1 }),
        ),
        (
            "workflowBoard.retryStep",
            json!({ "cardId": "card-missing", "expectedRevision": 1, "stepKind": "stage" }),
        ),
        (
            "workflowBoard.proposeAmendment",
            json!({ "cardId": "card-missing", "expectedRevision": 1, "reason": "Missing card" }),
        ),
    ] {
        let RpcExit::Failure { cause } = call_unary(&mut ws_op, "m3-op", method, payload).await?
        else {
            panic!("expected card_not_found for {method}");
        };
        let Some(RpcCauseReason::Fail { error }) = cause.first() else {
            panic!("expected Fail cause, got {cause:?}");
        };
        assert_eq!(error["_tag"], "WorkflowBoardRequestError");
        assert_eq!(error["code"], "card_not_found");
    }

    let mut ws_client = connect_ws(server, client_tok).await?;
    for (method, payload) in [
        (
            "workflowBoard.resolveFinding",
            json!({ "cardId": "card-missing", "expectedRevision": 1, "findingId": "fnd-1", "disposition": "fix" }),
        ),
        (
            "workflowBoard.recordManualAcceptance",
            json!({ "cardId": "card-missing", "expectedRevision": 1, "criterionId": "AC-1", "verdict": "pass" }),
        ),
        (
            "workflowBoard.approveDelivery",
            json!({ "cardId": "card-missing", "expectedRevision": 1 }),
        ),
    ] {
        let RpcExit::Failure { cause } =
            call_unary(&mut ws_client, "m3-cl", method, payload).await?
        else {
            panic!("expected card_not_found for {method}");
        };
        let Some(RpcCauseReason::Fail { error }) = cause.first() else {
            panic!("expected Fail cause, got {cause:?}");
        };
        assert_eq!(error["_tag"], "WorkflowBoardRequestError");
        assert_eq!(error["code"], "card_not_found");
    }

    Ok(())
}
