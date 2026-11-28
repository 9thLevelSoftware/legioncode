//! Streaming WS RPC handler for `workflowBoard.subscribe`.

use legion_contracts::workflow_board::{
    WorkflowBoardRequestError, WorkflowBoardRequestErrorCode, WorkflowBoardSubscribeInput,
};
use legion_protocol::rpc_codec::RpcExit;
use serde_json::{Value, json};
use tokio::sync::broadcast::error::RecvError;

use super::super::stream_ctx::StreamContext;
use crate::serve::rpc::rpc_fail_obj;
use crate::serve::ws_mcp::rpc_fail;

fn board_err_exit(err: &WorkflowBoardRequestError) -> RpcExit {
    rpc_fail_obj(serde_json::to_value(err).unwrap_or_else(|_| {
        json!({
            "_tag": "WorkflowBoardRequestError",
            "code": "internal_error",
            "message": err.message,
        })
    }))
}

pub(super) async fn stream_workflow_board(ctx: &StreamContext, payload: Value) -> RpcExit {
    let req: WorkflowBoardSubscribeInput = match serde_json::from_value(payload) {
        Ok(p) => p,
        Err(err) => return rpc_fail("ParseError", &format!("Invalid payload: {err}")),
    };
    let Some(ref board) = ctx.state.runtime_http.workflow_board else {
        return board_err_exit(&WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::InternalError,
            "Workflow board service is unavailable.",
        ));
    };

    // Subscribe BEFORE reading the initial snapshot to avoid missed transitions.
    let mut rx = board.subscribe_changes();
    let initial = match board.get_board_snapshot(&req.project_id) {
        Ok(snap) => snap,
        Err(err) => return board_err_exit(&err),
    };
    let Ok(initial_val) = serde_json::to_value(&initial) else {
        return board_err_exit(&WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::InternalError,
            "Failed to serialize workflow board snapshot.",
        ));
    };
    if !ctx.send_chunk(initial_val).await {
        return RpcExit::Success { value: Value::Null };
    }

    loop {
        match rx.recv().await {
            Ok(changed_project_id) if changed_project_id == req.project_id.as_str() => {
                if let Ok(snap) = board.get_board_snapshot(&req.project_id)
                    && let Ok(val) = serde_json::to_value(&snap)
                    && !ctx.send_chunk(val).await
                {
                    break;
                }
            }
            Ok(_) => {}
            Err(RecvError::Lagged(_)) => {
                if let Ok(snap) = board.get_board_snapshot(&req.project_id)
                    && let Ok(val) = serde_json::to_value(&snap)
                    && !ctx.send_chunk(val).await
                {
                    break;
                }
            }
            Err(RecvError::Closed) => break,
        }
    }
    RpcExit::Success { value: Value::Null }
}
