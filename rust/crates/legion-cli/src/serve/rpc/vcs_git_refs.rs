//! Git branch/ref creation, checkout guards, init, and HEAD branch query helpers.

use std::path::Path;

use legion_protocol::rpc_codec::RpcExit;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{AppState, CwdPayload, git_err, parse_payload, run_git_cmd};
use crate::serve::rpc::git_actions;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateRefPayload {
    cwd: String,
    #[serde(alias = "branch")]
    ref_name: String,
    #[serde(default)]
    switch_ref: Option<bool>,
}

pub(super) async fn vcs_create_ref(state: &AppState, payload: Value) -> RpcExit {
    let req: CreateRefPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if req.switch_ref.unwrap_or(false)
        && let Err(err) = git_actions::check_checkout_idle(state, &req.cwd)
    {
        return err;
    }
    let args: Vec<&str> = if req.switch_ref.unwrap_or(false) {
        vec!["checkout", "-b", &req.ref_name]
    } else {
        vec!["branch", &req.ref_name]
    };
    match run_git_cmd(Path::new(&req.cwd), &args).await {
        Ok(_) => RpcExit::Success {
            value: json!({ "refName": req.ref_name }),
        },
        Err((op, detail)) => git_err(&op, &req.cwd, &detail),
    }
}

pub(super) async fn vcs_switch_ref(state: &AppState, payload: Value) -> RpcExit {
    let req: CreateRefPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if let Err(err) = git_actions::check_checkout_idle(state, &req.cwd) {
        return err;
    }
    match run_git_cmd(Path::new(&req.cwd), &["checkout", &req.ref_name]).await {
        Ok(_) => RpcExit::Success {
            value: json!({ "refName": req.ref_name }),
        },
        Err((op, detail)) => git_err(&op, &req.cwd, &detail),
    }
}

pub(super) async fn vcs_init(payload: Value) -> RpcExit {
    let req: CwdPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    match run_git_cmd(Path::new(&req.cwd), &["init"]).await {
        Ok(_) => RpcExit::Success { value: Value::Null },
        Err((op, detail)) => git_err(&op, &req.cwd, &detail),
    }
}

pub(crate) async fn current_branch(cwd: &Path) -> Option<String> {
    let txt = run_git_cmd(cwd, &["rev-parse", "--abbrev-ref", "HEAD"])
        .await
        .ok()?;
    let trimmed = txt.trim();
    if trimmed.is_empty() || trimmed == "HEAD" {
        None
    } else {
        Some(trimmed.to_owned())
    }
}
