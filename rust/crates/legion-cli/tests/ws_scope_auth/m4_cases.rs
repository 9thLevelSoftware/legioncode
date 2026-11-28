use super::helpers::{
    TestResult, assert_auth_denied, call_unary, connect_ws, issue_token, pair_standard_client,
    spawn_srv,
};
use legion_protocol::rpc_codec::{RpcCauseReason, RpcExit};
use serde_json::json;

#[tokio::test]
async fn ws_enforces_m4_client_only_and_scope_guards() -> TestResult {
    let temp = tempfile::tempdir()?;
    let server = spawn_srv(&temp.path().join("m4-ws-scope")).await?;

    let client_tok = pair_standard_client(&server).await?;
    let read_sess = issue_token(&server, "thread:thr-m4-read", &["orchestration:read"])?;
    let thread_op = issue_token(&server, "thread:thr-m4-op", &["orchestration:operate"])?;
    let wildcard_agent = issue_token(&server, "thread:thr-m4-wild", &["*"])?;
    let mcp_oauth = issue_token(&server, "mcp-oauth:sess-m4", &["*"])?;
    let peer_link = issue_token(&server, "peer-link:peer-m4", &["*"])?;
    let client_read = issue_token(&server, "client:m4-read-only", &["orchestration:read"])?;

    // Agent/MCP/Peer callers cannot invoke `workflowBoard.applyCliImport` (human-client-only).
    for tok in [&thread_op, &wildcard_agent, &mcp_oauth, &peer_link] {
        let mut ws = connect_ws(&server, tok).await?;
        let RpcExit::Failure { cause } = call_unary(
            &mut ws,
            "m4-cg",
            "workflowBoard.applyCliImport",
            json!({ "projectId": "proj-missing" }),
        )
        .await?
        else {
            panic!("expected client-only denial for applyCliImport");
        };
        let Some(RpcCauseReason::Fail { error }) = cause.first() else {
            panic!("expected Fail cause, got {cause:?}");
        };
        assert_eq!(error["_tag"], "EnvironmentAuthorizationError");
    }

    assert_m4_read_denials(&server, &read_sess, &client_read).await?;

    // Non-client caller with `orchestration:operate` CAN call `previewCliImport` (hits domain NotFound).
    let mut ws_op = connect_ws(&server, &thread_op).await?;
    let RpcExit::Failure { cause } = call_unary(
        &mut ws_op,
        "m4-op-preview",
        "workflowBoard.previewCliImport",
        json!({ "projectId": "proj-missing" }),
    )
    .await?
    else {
        panic!("expected domain error for missing project");
    };
    let Some(RpcCauseReason::Fail { error }) = cause.first() else {
        panic!("expected Fail cause, got {cause:?}");
    };
    assert_eq!(error["_tag"], "WorkflowBoardRequestError");
    assert_eq!(error["code"], "project_not_found");

    // Standard client with `orchestration:operate` passes auth on both `previewCliImport` and `applyCliImport`.
    let mut ws_client = connect_ws(&server, &client_tok).await?;
    for (method, payload) in [
        (
            "workflowBoard.previewCliImport",
            json!({ "projectId": "proj-missing" }),
        ),
        (
            "workflowBoard.applyCliImport",
            json!({ "projectId": "proj-missing" }),
        ),
    ] {
        let RpcExit::Failure { cause } =
            call_unary(&mut ws_client, "m4-cl", method, payload).await?
        else {
            panic!("expected domain error for {method}");
        };
        let Some(RpcCauseReason::Fail { error }) = cause.first() else {
            panic!("expected Fail cause, got {cause:?}");
        };
        assert_eq!(error["_tag"], "WorkflowBoardRequestError");
        assert_eq!(error["code"], "project_not_found");
    }

    server.shutdown().await?;
    Ok(())
}

async fn assert_m4_read_denials(
    server: &legion_cli::serve::RunningServer,
    read_sess: &str,
    client_read: &str,
) -> TestResult {
    let mut ws_read = connect_ws(server, read_sess).await?;
    let exit_preview = call_unary(
        &mut ws_read,
        "m4-r1",
        "workflowBoard.previewCliImport",
        json!({ "projectId": "proj-missing" }),
    )
    .await?;
    assert_auth_denied(
        exit_preview,
        "orchestration:operate",
        "orchestration:operate",
    );

    let exit_apply_agent = call_unary(
        &mut ws_read,
        "m4-r2",
        "workflowBoard.applyCliImport",
        json!({ "projectId": "proj-missing" }),
    )
    .await?;
    assert_auth_denied(
        exit_apply_agent,
        "orchestration:operate",
        "client:interactive",
    );

    let mut ws_client_read = connect_ws(server, client_read).await?;
    let exit_apply_client_read = call_unary(
        &mut ws_client_read,
        "m4-cr",
        "workflowBoard.applyCliImport",
        json!({ "projectId": "proj-missing" }),
    )
    .await?;
    assert_auth_denied(
        exit_apply_client_read,
        "orchestration:operate",
        "orchestration:operate",
    );
    Ok(())
}
