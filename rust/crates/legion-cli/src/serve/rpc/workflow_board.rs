//! Unary WS RPC handlers for `workflowBoard.{getCard,getArtifact,createCard,updateSpec,approveSpec,startStage}`.

use std::sync::Arc;

use legion_contracts::workflow_board::{
    WorkflowBoardApprovePlanInput, WorkflowBoardApproveSpecInput, WorkflowBoardChallengeStepInput,
    WorkflowBoardCreateCardInput, WorkflowBoardExploreStepInput, WorkflowBoardGetArtifactInput,
    WorkflowBoardGetCardInput, WorkflowBoardRequestError, WorkflowBoardRequestErrorCode,
    WorkflowBoardReturnForRevisionInput, WorkflowBoardStartStageInput,
    WorkflowBoardUpdatePlanInput, WorkflowBoardUpdateSpecInput,
};
use legion_protocol::rpc_codec::RpcExit;
use legion_server_runtime::ProjectWorkflowBoardService;
use serde::Serialize;
use serde_json::{Value, json};

use super::{AppState, parse_payload, rpc_fail, rpc_fail_obj};

pub(crate) async fn dispatch(state: &Arc<AppState>, tag: &str, payload: Value) -> RpcExit {
    let Some(ref board) = state.runtime_http.workflow_board else {
        return board_err(&WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::InternalError,
            "Workflow board service is unavailable.",
        ));
    };
    match tag {
        "workflowBoard.getCard" => handle_get_card(board, payload),
        "workflowBoard.getArtifact" => handle_get_artifact(board, payload),
        "workflowBoard.createCard" => handle_create_card(board, payload),
        "workflowBoard.updateSpec" => handle_update_spec(board, payload).await,
        "workflowBoard.approveSpec" => handle_approve_spec(board, payload).await,
        "workflowBoard.startStage" => handle_start_stage(board, payload).await,
        "workflowBoard.exploreStep" => handle_explore_step(board, payload).await,
        "workflowBoard.challengeStep" => handle_challenge_step(board, payload).await,
        "workflowBoard.updatePlan" => handle_update_plan(board, payload).await,
        "workflowBoard.approvePlan" => handle_approve_plan(board, payload).await,
        "workflowBoard.returnForRevision" => handle_return_for_revision(board, payload).await,
        "workflowBoard.pauseCard"
        | "workflowBoard.resumeCard"
        | "workflowBoard.retryStep"
        | "workflowBoard.proposeAmendment"
        | "workflowBoard.resolveFinding"
        | "workflowBoard.recordManualAcceptance"
        | "workflowBoard.approveDelivery"
        | "workflowBoard.getCardDiff" => {
            super::workflow_board_m3::dispatch(board, tag, payload).await
        }
        "workflowBoard.previewCliImport" | "workflowBoard.applyCliImport" => {
            super::workflow_board_m4::dispatch(board, tag, payload)
        }
        _ => rpc_fail(
            "UnsupportedRpcMethod",
            &format!("Unsupported RPC method `{tag}`"),
        ),
    }
}

pub(super) fn board_err(err: &WorkflowBoardRequestError) -> RpcExit {
    rpc_fail_obj(serde_json::to_value(err).unwrap_or_else(|_| {
        json!({
            "_tag": "WorkflowBoardRequestError",
            "code": "internal_error",
            "message": err.message,
        })
    }))
}

pub(super) fn to_rpc_result<T: Serialize>(res: Result<T, WorkflowBoardRequestError>) -> RpcExit {
    match res {
        Ok(val) => match serde_json::to_value(val) {
            Ok(value) => RpcExit::Success { value },
            Err(err) => board_err(&WorkflowBoardRequestError::new(
                WorkflowBoardRequestErrorCode::InternalError,
                err.to_string(),
            )),
        },
        Err(err) => board_err(&err),
    }
}

fn handle_get_card(board: &ProjectWorkflowBoardService, payload: Value) -> RpcExit {
    let req: WorkflowBoardGetCardInput = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    to_rpc_result(board.get_card_detail(&req))
}

fn handle_get_artifact(board: &ProjectWorkflowBoardService, payload: Value) -> RpcExit {
    let req: WorkflowBoardGetArtifactInput = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    to_rpc_result(board.get_artifact(&req))
}

fn handle_create_card(board: &ProjectWorkflowBoardService, payload: Value) -> RpcExit {
    let req: WorkflowBoardCreateCardInput = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    to_rpc_result(board.create_card(&req))
}

async fn handle_update_spec(board: &ProjectWorkflowBoardService, payload: Value) -> RpcExit {
    let req: WorkflowBoardUpdateSpecInput = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    to_rpc_result(board.update_spec(&req).await)
}

async fn handle_approve_spec(board: &ProjectWorkflowBoardService, payload: Value) -> RpcExit {
    let req: WorkflowBoardApproveSpecInput = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    to_rpc_result(board.approve_spec(&req, "client").await)
}

async fn handle_start_stage(board: &ProjectWorkflowBoardService, payload: Value) -> RpcExit {
    let req: WorkflowBoardStartStageInput = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    to_rpc_result(board.start_stage(&req).await)
}

async fn handle_explore_step(board: &ProjectWorkflowBoardService, payload: Value) -> RpcExit {
    let req: WorkflowBoardExploreStepInput = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    to_rpc_result(board.explore_step(&req).await)
}

async fn handle_challenge_step(board: &ProjectWorkflowBoardService, payload: Value) -> RpcExit {
    let req: WorkflowBoardChallengeStepInput = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    to_rpc_result(board.challenge_step(&req).await)
}

async fn handle_update_plan(board: &ProjectWorkflowBoardService, payload: Value) -> RpcExit {
    let req: WorkflowBoardUpdatePlanInput = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    to_rpc_result(board.update_plan(&req).await)
}

async fn handle_approve_plan(board: &ProjectWorkflowBoardService, payload: Value) -> RpcExit {
    let req: WorkflowBoardApprovePlanInput = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    to_rpc_result(board.approve_plan(&req, "client").await)
}

async fn handle_return_for_revision(
    board: &ProjectWorkflowBoardService,
    payload: Value,
) -> RpcExit {
    let req: WorkflowBoardReturnForRevisionInput = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    to_rpc_result(board.return_for_revision(&req).await)
}
