//! Source-control host and project clone RPC handlers (`sourceControl.*`, `projectClone.*`).

use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;

use legion_protocol::rpc_codec::RpcExit;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::process::Command;
use uuid::Uuid;

use super::projects_fs::base_dir_from_state;
use super::scm_issues::run_gh_json;
use super::{AppState, parse_payload, rpc_fail, rpc_fail_obj};

pub(crate) async fn dispatch(state: &Arc<AppState>, tag: &str, payload: Value) -> RpcExit {
    match tag {
        "sourceControl.lookupRepository" => source_control_lookup(payload).await,
        "sourceControl.cloneRepository" => source_control_clone(state, payload).await,
        "sourceControl.publishRepository" => source_control_publish(payload).await,
        "sourceControl.signInStart" => RpcExit::Success {
            value: json!({
                "status": "pending",
                "verificationUrl": "https://github.com/login/device",
            }),
        },
        "sourceControl.signInWait" => RpcExit::Success {
            value: json!({ "status": "cancelled" }),
        },
        "sourceControl.signInCancel" | "sourceControl.star" => {
            RpcExit::Success { value: json!({}) }
        }
        "sourceControl.appAccess" => RpcExit::Success {
            value: json!({ "installed": false, "repositories": [] }),
        },
        "sourceControl.starred" => RpcExit::Success {
            value: json!({ "repositories": [] }),
        },
        "projectClone.start" | "projectClone.retry" => project_clone_start(state, payload).await,
        "projectClone.cancel" => RpcExit::Success {
            value: json!({ "cancelled": false }),
        },
        _ => rpc_fail(
            "UnsupportedRpcMethod",
            &format!("Unsupported RPC method `{tag}`"),
        ),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LookupRepoPayload {
    repository: String,
}

async fn source_control_lookup(payload: Value) -> RpcExit {
    let req: LookupRepoPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    match run_gh_json(
        None,
        &[
            "repo",
            "view",
            &req.repository,
            "--json",
            "name,nameWithOwner,url,defaultBranchRef,isPrivate",
        ],
    )
    .await
    {
        Ok(val) => RpcExit::Success {
            value: json!({ "repository": val }),
        },
        Err(err) => rpc_fail_obj(json!({ "_tag": "SourceControlError", "message": err })),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CloneRepoPayload {
    #[serde(alias = "repositoryUrl", alias = "remoteUrl")]
    url: String,
    target_directory: Option<String>,
}

async fn source_control_clone(state: &AppState, payload: Value) -> RpcExit {
    let req: CloneRepoPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let dest = req.target_directory.unwrap_or_else(|| {
        base_dir_from_state(state)
            .join("clones")
            .join(format!(
                "clone-{}",
                &Uuid::new_v4().simple().to_string()[..8]
            ))
            .to_string_lossy()
            .into_owned()
    });
    if let Some(parent) = Path::new(&dest).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let out = Command::new("git")
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(["clone", &req.url, &dest])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await;
    match out {
        Ok(res) if res.status.success() => RpcExit::Success {
            value: json!({ "workspaceRoot": dest }),
        },
        Ok(res) => rpc_fail_obj(json!({
            "_tag": "SourceControlError",
            "message": String::from_utf8_lossy(&res.stderr).trim(),
        })),
        Err(e) => rpc_fail_obj(json!({ "_tag": "SourceControlError", "message": e.to_string() })),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PublishRepoPayload {
    cwd: String,
    name: String,
    #[serde(default)]
    private: Option<bool>,
}

async fn source_control_publish(payload: Value) -> RpcExit {
    let req: PublishRepoPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let visibility = if req.private.unwrap_or(true) {
        "--private"
    } else {
        "--public"
    };
    let out = Command::new("gh")
        .current_dir(Path::new(&req.cwd))
        .env("GIT_TERMINAL_PROMPT", "0")
        .args([
            "repo",
            "create",
            &req.name,
            visibility,
            "--source=.",
            "--push",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await;
    match out {
        Ok(res) if res.status.success() => RpcExit::Success {
            value: json!({ "url": String::from_utf8_lossy(&res.stdout).trim() }),
        },
        Ok(res) => rpc_fail_obj(json!({
            "_tag": "SourceControlError",
            "message": String::from_utf8_lossy(&res.stderr).trim(),
        })),
        Err(e) => rpc_fail_obj(json!({ "_tag": "SourceControlError", "message": e.to_string() })),
    }
}

async fn project_clone_start(state: &AppState, payload: Value) -> RpcExit {
    let clone_id = format!("clone-{}", Uuid::new_v4());
    match source_control_clone(state, payload).await {
        RpcExit::Success { value } => RpcExit::Success {
            value: json!({
                "cloneId": clone_id,
                "status": "completed",
                "workspaceRoot": value["workspaceRoot"],
            }),
        },
        err @ RpcExit::Failure { .. } => err,
    }
}
