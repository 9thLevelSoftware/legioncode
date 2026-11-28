//! Unary WS RPC handlers for M4 `.legion-cli/` import preview and apply.

use legion_contracts::workflow_board::{
    WorkflowBoardApplyCliImportInput, WorkflowBoardPreviewCliImportInput,
    WorkflowBoardRequestError, WorkflowBoardRequestErrorCode,
};
use legion_protocol::rpc_codec::RpcExit;
use legion_server_runtime::ProjectWorkflowBoardService;
use serde_json::Value;

use super::parse_payload;
use super::workflow_board::{board_err, to_rpc_result};

pub(super) fn dispatch(board: &ProjectWorkflowBoardService, tag: &str, payload: Value) -> RpcExit {
    match tag {
        "workflowBoard.previewCliImport" => {
            match parse_payload::<WorkflowBoardPreviewCliImportInput>(payload) {
                Ok(req) => to_rpc_result(board.preview_cli_import(&req)),
                Err(e) => e,
            }
        }
        "workflowBoard.applyCliImport" => {
            match parse_payload::<WorkflowBoardApplyCliImportInput>(payload) {
                Ok(req) => to_rpc_result(board.apply_cli_import(&req)),
                Err(e) => e,
            }
        }
        _ => board_err(&WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::InternalError,
            format!("Unsupported M4 workflowBoard RPC `{tag}`"),
        )),
    }
}
