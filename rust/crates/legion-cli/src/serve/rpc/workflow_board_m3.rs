//! Unary WS RPC handlers for M3 workflow-board execution, recovery, diff, and ship gates.

use legion_contracts::workflow_board::{
    WorkflowBoardApproveDeliveryInput, WorkflowBoardGetCardDiffInput, WorkflowBoardPauseCardInput,
    WorkflowBoardProposeAmendmentInput, WorkflowBoardRecordManualAcceptanceInput,
    WorkflowBoardResolveFindingInput, WorkflowBoardResumeCardInput, WorkflowBoardRetryStepInput,
};
use legion_protocol::rpc_codec::RpcExit;
use legion_server_runtime::ProjectWorkflowBoardService;
use serde_json::Value;

use super::parse_payload;
use super::workflow_board::{board_err, to_rpc_result};

pub(super) async fn dispatch(
    board: &ProjectWorkflowBoardService,
    tag: &str,
    payload: Value,
) -> RpcExit {
    match tag {
        "workflowBoard.pauseCard" => match parse_payload::<WorkflowBoardPauseCardInput>(payload) {
            Ok(req) => to_rpc_result(board.pause_card(&req).await),
            Err(e) => e,
        },
        "workflowBoard.resumeCard" => {
            match parse_payload::<WorkflowBoardResumeCardInput>(payload) {
                Ok(req) => to_rpc_result(board.resume_card(&req).await),
                Err(e) => e,
            }
        }
        "workflowBoard.retryStep" => match parse_payload::<WorkflowBoardRetryStepInput>(payload) {
            Ok(req) => to_rpc_result(board.retry_step(&req).await),
            Err(e) => e,
        },
        "workflowBoard.proposeAmendment" => {
            match parse_payload::<WorkflowBoardProposeAmendmentInput>(payload) {
                Ok(req) => to_rpc_result(board.propose_amendment(&req, "client").await),
                Err(e) => e,
            }
        }
        "workflowBoard.resolveFinding" => {
            match parse_payload::<WorkflowBoardResolveFindingInput>(payload) {
                Ok(req) => to_rpc_result(board.resolve_finding(&req, "client").await),
                Err(e) => e,
            }
        }
        "workflowBoard.recordManualAcceptance" => {
            match parse_payload::<WorkflowBoardRecordManualAcceptanceInput>(payload) {
                Ok(req) => to_rpc_result(board.record_manual_acceptance(&req, "client").await),
                Err(e) => e,
            }
        }
        "workflowBoard.approveDelivery" => {
            match parse_payload::<WorkflowBoardApproveDeliveryInput>(payload) {
                Ok(req) => to_rpc_result(board.approve_delivery(&req, "client").await),
                Err(e) => e,
            }
        }
        "workflowBoard.getCardDiff" => {
            match parse_payload::<WorkflowBoardGetCardDiffInput>(payload) {
                Ok(req) => to_rpc_result(board.get_card_diff(&req)),
                Err(e) => e,
            }
        }
        _ => board_err(
            &legion_contracts::workflow_board::WorkflowBoardRequestError::new(
                legion_contracts::workflow_board::WorkflowBoardRequestErrorCode::InternalError,
                format!("Unsupported M3 workflowBoard RPC `{tag}`"),
            ),
        ),
    }
}
