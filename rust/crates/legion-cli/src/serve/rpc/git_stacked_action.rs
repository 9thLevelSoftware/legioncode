//! Live `git.runStackedAction` RPC and progress-streaming bridge.

use std::path::PathBuf;

use legion_protocol::rpc_codec::RpcExit;
use legion_scm::stacked_action::{
    StackedActionKind, StackedActionPhase, StackedActionRequest, execute_stacked_action,
    phases_for_action,
};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::mpsc;

use super::parse_payload;
use super::vcs_git::git_err;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StackedActionPayload {
    cwd: String,
    action: String,
    commit_message: Option<String>,
    #[serde(default)]
    feature_branch: Option<bool>,
    #[serde(default)]
    file_paths: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
pub(crate) struct StackedProgressChunk {
    pub phase: &'static str,
    pub label: String,
}

#[must_use]
pub(crate) fn stacked_action_phases(payload: &Value) -> Vec<&'static str> {
    let action_raw = payload
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("commit");
    let feature_branch = payload
        .get("featureBranch")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    StackedActionKind::parse(action_raw).map_or_default(|k| {
        phases_for_action(k, feature_branch)
            .into_iter()
            .map(StackedActionPhase::as_str)
            .collect()
    })
}

pub(crate) async fn git_run_stacked_action(payload: Value) -> RpcExit {
    run_stacked_action_with_progress(payload, None).await.0
}

pub(crate) async fn run_stacked_action_with_progress(
    payload: Value,
    progress_tx: Option<mpsc::UnboundedSender<StackedProgressChunk>>,
) -> (RpcExit, Option<&'static str>) {
    let req: StackedActionPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return (e, None),
    };
    let Some(kind) = StackedActionKind::parse(&req.action) else {
        return (
            git_err(
                "git.runStackedAction",
                &req.cwd,
                &format!("Unsupported stacked git action `{}`", req.action),
            ),
            None,
        );
    };
    let scm_req = StackedActionRequest {
        cwd: PathBuf::from(&req.cwd),
        action: kind,
        commit_message: req.commit_message,
        feature_branch: req.feature_branch.unwrap_or(false),
        file_paths: req.file_paths,
    };
    let mut last_phase: Option<StackedActionPhase> = None;
    let outcome = execute_stacked_action(&scm_req, |phase, label| {
        last_phase = Some(phase);
        if let Some(ref tx) = progress_tx {
            let _ = tx.send(StackedProgressChunk {
                phase: phase.as_str(),
                label: label.to_owned(),
            });
        }
    })
    .await;

    match outcome {
        Ok(ok) => match serde_json::to_value(ok) {
            Ok(value) => (RpcExit::Success { value }, None),
            Err(e) => (
                git_err("git.runStackedAction", &req.cwd, &e.to_string()),
                last_phase.map(StackedActionPhase::as_str),
            ),
        },
        Err(err) => {
            let phase_str = err.phase.or(last_phase).map(StackedActionPhase::as_str);
            (git_err(&err.operation, &req.cwd, &err.detail), phase_str)
        }
    }
}
