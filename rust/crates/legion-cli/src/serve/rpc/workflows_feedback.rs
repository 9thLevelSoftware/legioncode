//! Unary workflow RPC handlers (`workflows.{get,answer,resume,cancel,*Definition}`) and
//! local-only agent feedback RPCs (`agentFeedback.answerRequest`, `sent = 0`, `ADR-013`).

use legion_contracts::agent_feedback::{AgentFeedbackAnswerChoice, AgentFeedbackAnswerInput};
use legion_contracts::workflows::{
    WorkflowAnswerInput, WorkflowDefinitionDeleteInput, WorkflowDefinitionSaveInput,
    WorkflowRunCancelInput, WorkflowRunGetInput, WorkflowRunResumeInput,
};
use legion_protocol::rpc_codec::RpcExit;
use legion_workflows::feedback_types::{
    AgentFeedbackAnswerRequest, FeedbackAnswerChoice, FeedbackError, FeedbackMode,
};
use serde_json::{Value, json};

use super::server_settings::read_settings_json;
use super::{AppState, parse_payload, rpc_fail, rpc_fail_obj};

pub(crate) async fn dispatch(state: &AppState, tag: &str, payload: Value) -> RpcExit {
    match tag {
        "workflows.get" => wf_get(state, payload),
        "workflows.answer" => wf_answer(state, payload).await,
        "workflows.resume" => wf_resume(state, payload).await,
        "workflows.cancel" => wf_cancel(state, payload).await,
        "workflows.getDefinition" => wf_get_definition(state),
        "workflows.saveDefinition" => wf_save_definition(state, payload),
        "workflows.deleteDefinition" => wf_delete_definition(state, payload),
        "agentFeedback.answerRequest" => answer_feedback_request(state, payload),
        _ => rpc_fail(
            "UnsupportedRpcMethod",
            &format!("Unsupported RPC method `{tag}`"),
        ),
    }
}

fn wf_err(message: &str) -> RpcExit {
    rpc_fail_obj(json!({
        "_tag": "WorkflowRequestError",
        "message": message,
    }))
}

fn wf_get(state: &AppState, payload: Value) -> RpcExit {
    let req: WorkflowRunGetInput = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let Some(engine) = state.runtime_http.workflow_engine() else {
        return wf_err("Workflow runtime is unavailable.");
    };
    match engine.get(&req.run_id) {
        Ok(detail) => RpcExit::Success {
            value: json!(detail),
        },
        Err(err) => wf_err(&err.to_string()),
    }
}

fn build_workflow_answer_payload(choice: &str, reason: Option<&str>) -> Value {
    let mut answer = json!({ "choice": choice });
    if let Some(r) = reason {
        answer["reason"] = Value::String(r.to_owned());
    }
    answer
}

async fn wf_answer(state: &AppState, payload: Value) -> RpcExit {
    let req: WorkflowAnswerInput = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let Some(engine) = state.runtime_http.workflow_engine() else {
        return wf_err("Workflow runtime is unavailable.");
    };
    let answer = build_workflow_answer_payload(&req.choice, req.reason.as_deref());
    match engine.answer(&req.run_id, &req.name, answer).await {
        Ok(()) => RpcExit::Success { value: Value::Null },
        Err(err) => wf_err(&err.to_string()),
    }
}

async fn wf_resume(state: &AppState, payload: Value) -> RpcExit {
    let req: WorkflowRunResumeInput = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let Some(engine) = state.runtime_http.workflow_engine() else {
        return wf_err("Workflow runtime is unavailable.");
    };
    match engine.resume(&req.run_id).await {
        Ok(()) => RpcExit::Success { value: Value::Null },
        Err(err) => wf_err(&err.to_string()),
    }
}

async fn wf_cancel(state: &AppState, payload: Value) -> RpcExit {
    let req: WorkflowRunCancelInput = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let Some(engine) = state.runtime_http.workflow_engine() else {
        return wf_err("Workflow runtime is unavailable.");
    };
    match engine.cancel(&req.run_id, req.reason).await {
        Ok(()) => RpcExit::Success { value: Value::Null },
        Err(err) => wf_err(&err.to_string()),
    }
}

fn empty_workflow_definition(dir: &std::path::Path) -> Value {
    json!({
        "directory": dir.join("workflows").to_string_lossy(),
        "single": false,
        "files": [],
        "problems": [],
        "revision": 0,
        "problem": null,
    })
}

fn wf_get_definition(state: &AppState) -> RpcExit {
    let Some(ref mgr) = state.runtime_http.workflow_manager else {
        return RpcExit::Success {
            value: empty_workflow_definition(&state.paths.state_dir),
        };
    };
    RpcExit::Success {
        value: json!(mgr.read()),
    }
}

fn wf_save_definition(state: &AppState, payload: Value) -> RpcExit {
    let req: WorkflowDefinitionSaveInput = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let Some(ref mgr) = state.runtime_http.workflow_manager else {
        return RpcExit::Success {
            value: json!({
                "changed": false,
                "problems": [{ "message": "Workflow runtime is unavailable." }],
            }),
        };
    };
    match mgr.save(&req.name, &req.source) {
        Ok(res) => RpcExit::Success { value: json!(res) },
        Err(err) => RpcExit::Success {
            value: json!({
                "changed": false,
                "problems": [{ "message": err.to_string(), "file": req.name }],
            }),
        },
    }
}

fn wf_delete_definition(state: &AppState, payload: Value) -> RpcExit {
    let req: WorkflowDefinitionDeleteInput = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let Some(ref mgr) = state.runtime_http.workflow_manager else {
        return RpcExit::Success {
            value: json!({
                "changed": false,
                "problems": [{ "message": "Workflow runtime is unavailable." }],
            }),
        };
    };
    match mgr.remove(&req.name) {
        Ok(res) => RpcExit::Success { value: json!(res) },
        Err(err) => RpcExit::Success {
            value: json!({
                "changed": false,
                "problems": [{ "message": err.to_string(), "file": req.name }],
            }),
        },
    }
}

fn map_feedback_error_reason(err: &FeedbackError) -> &'static str {
    match err {
        FeedbackError::NotFound | FeedbackError::NotRequested { .. } => "not_found",
        FeedbackError::AlreadyAnswered => "already_answered",
        FeedbackError::RequestFeedbackOff | FeedbackError::Off => "feedback_off",
        FeedbackError::LoadFailed => "load_failed",
        FeedbackError::LimitReached { .. }
        | FeedbackError::InvalidSubject
        | FeedbackError::ApprovalContextMissing
        | FeedbackError::Storage(_) => "record_failed",
    }
}

fn answer_feedback_request(state: &AppState, payload: Value) -> RpcExit {
    let req: AgentFeedbackAnswerInput = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let Some(ref store) = state.runtime_http.feedback_store else {
        return rpc_fail_obj(json!({
            "_tag": "AgentFeedbackRequestError",
            "reason": "load_failed",
        }));
    };
    let mode = read_feedback_mode(state);
    let inner = AgentFeedbackAnswerRequest {
        thread_id: req.thread_id.as_str().to_owned(),
        turn_item_id: req.turn_item_id.as_str().to_owned(),
        answer: match req.answer {
            AgentFeedbackAnswerChoice::Send => FeedbackAnswerChoice::Send,
            AgentFeedbackAnswerChoice::Decline => FeedbackAnswerChoice::Decline,
        },
        include_conversation: req.include_conversation.unwrap_or(false),
    };
    match store.answer_request(mode, &inner) {
        Ok(_) => RpcExit::Success { value: Value::Null },
        Err(err) => rpc_fail_obj(json!({
            "_tag": "AgentFeedbackRequestError",
            "reason": map_feedback_error_reason(&err),
        })),
    }
}

pub(crate) fn read_feedback_mode(state: &AppState) -> FeedbackMode {
    let Ok(settings) = read_settings_json(&state.paths.settings_path) else {
        return FeedbackMode::Off;
    };
    match settings
        .pointer("/agentFeedback/mode")
        .and_then(Value::as_str)
    {
        Some("on-request") => FeedbackMode::OnRequest,
        Some("auto") => FeedbackMode::Auto,
        _ => FeedbackMode::Off,
    }
}

#[cfg(test)]
mod tests {
    use legion_contracts::agent_feedback::AgentFeedbackFailureReason;
    use legion_workflows::feedback_types::{FeedbackError, FeedbackSource};
    use serde_json::{Value, json};

    use super::{
        build_workflow_answer_payload, empty_workflow_definition, map_feedback_error_reason,
    };

    #[test]
    fn answer_payload_definition_fallback_and_feedback_reasons_match_contracts()
    -> Result<(), serde_json::Error> {
        assert_eq!(
            build_workflow_answer_payload("approve", Some("looks good")),
            json!({ "choice": "approve", "reason": "looks good" })
        );
        assert_eq!(
            build_workflow_answer_payload("approve", None),
            json!({ "choice": "approve" })
        );

        let fallback = empty_workflow_definition(std::path::Path::new("/tmp/state"));
        assert_eq!(fallback["revision"], 0);
        assert_eq!(fallback.get("problem"), Some(&Value::Null));

        let samples = [
            FeedbackError::NotFound,
            FeedbackError::NotRequested {
                origin: FeedbackSource::Mcp,
            },
            FeedbackError::AlreadyAnswered,
            FeedbackError::RequestFeedbackOff,
            FeedbackError::Off,
            FeedbackError::LoadFailed,
            FeedbackError::LimitReached { scope: "thread" },
            FeedbackError::InvalidSubject,
            FeedbackError::ApprovalContextMissing,
            FeedbackError::Storage("io".to_owned()),
        ];
        for err in &samples {
            let wire = map_feedback_error_reason(err);
            let parsed: AgentFeedbackFailureReason =
                serde_json::from_value(Value::String(wire.to_owned()))?;
            assert_eq!(
                serde_json::to_value(parsed)?,
                Value::String(wire.to_owned())
            );
        }
        Ok(())
    }
}
