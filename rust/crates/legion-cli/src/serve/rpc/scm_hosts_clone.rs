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
    let provider = payload
        .get("provider")
        .and_then(Value::as_str)
        .unwrap_or("github");
    let host = payload
        .get("host")
        .and_then(Value::as_str)
        .unwrap_or("github.com");
    match tag {
        "sourceControl.lookupRepository" => source_control_lookup(payload).await,
        "sourceControl.cloneRepository" => source_control_clone(state, payload).await,
        "sourceControl.publishRepository" => source_control_publish(payload).await,
        "sourceControl.signInStart" | "sourceControl.signInWait" => rpc_fail_obj(json!({
            "_tag": "SourceControlSignInError",
            "provider": provider,
            "host": host,
            "reason": "unsupported",
            "detail": "Interactive forge sign-in is not available in this runtime.",
        })),
        "sourceControl.signInCancel" => RpcExit::Success { value: Value::Null },
        "sourceControl.appAccess" => RpcExit::Success {
            value: json!({ "status": "signed-out" }),
        },
        "sourceControl.starred" | "sourceControl.star" => RpcExit::Success {
            value: json!(false),
        },
        "projectClone.start" => project_clone_start(state, payload).await,
        "projectClone.cancel" | "projectClone.retry" => RpcExit::Success {
            value: json!({ "applied": false }),
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
    #[serde(default)]
    provider: Option<String>,
    repository: String,
}

async fn source_control_lookup(payload: Value) -> RpcExit {
    let req: LookupRepoPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let provider = req.provider.unwrap_or_else(|| "github".to_owned());
    match run_gh_json(
        None,
        &[
            "repo",
            "view",
            &req.repository,
            "--json",
            "nameWithOwner,url,sshUrl",
        ],
    )
    .await
    {
        Ok(val) => RpcExit::Success {
            value: json!({
                "provider": provider,
                "nameWithOwner": val.get("nameWithOwner").and_then(Value::as_str).unwrap_or(&req.repository),
                "url": val.get("url").and_then(Value::as_str).unwrap_or(""),
                "sshUrl": val.get("sshUrl").cloned().unwrap_or(Value::Null),
            }),
        },
        Err(err) => rpc_fail_obj(json!({
            "_tag": "SourceControlRepositoryError",
            "provider": provider,
            "operation": "lookupRepository",
            "detail": err,
        })),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct CloneRepoPayload {
    provider: Option<String>,
    url: Option<String>,
    repository_url: Option<String>,
    remote_url: Option<String>,
    repository: Option<String>,
    target_directory: Option<String>,
    destination_path: Option<String>,
}

async fn source_control_clone(state: &AppState, payload: Value) -> RpcExit {
    let req: CloneRepoPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let provider = req.provider.unwrap_or_else(|| "github".to_owned());
    let clone_url = req
        .remote_url
        .or(req.repository_url)
        .or(req.url)
        .or(req.repository)
        .unwrap_or_default();
    let dest = req
        .destination_path
        .or(req.target_directory)
        .unwrap_or_else(|| {
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
        .args(["clone", &clone_url, &dest])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await;
    match out {
        Ok(res) if res.status.success() => RpcExit::Success {
            value: json!({
                "cwd": dest,
                "remoteUrl": clone_url,
                "repository": Value::Null,
            }),
        },
        Ok(res) => rpc_fail_obj(json!({
            "_tag": "SourceControlRepositoryError",
            "provider": provider,
            "operation": "cloneRepository",
            "detail": String::from_utf8_lossy(&res.stderr).trim(),
        })),
        Err(e) => rpc_fail_obj(json!({
            "_tag": "SourceControlRepositoryError",
            "provider": provider,
            "operation": "cloneRepository",
            "detail": e.to_string(),
        })),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PublishRepoPayload {
    cwd: String,
    #[serde(default)]
    provider: Option<String>,
    #[serde(alias = "repository")]
    name: String,
    #[serde(default)]
    visibility: Option<String>,
    #[serde(default)]
    remote_name: Option<String>,
}

async fn source_control_publish(payload: Value) -> RpcExit {
    let req: PublishRepoPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let provider = req.provider.unwrap_or_else(|| "github".to_owned());
    let visibility = if req.visibility.as_deref() == Some("public") {
        "--public"
    } else {
        "--private"
    };
    let remote_name = req.remote_name.unwrap_or_else(|| "origin".to_owned());
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
        Ok(res) if res.status.success() => {
            let url = String::from_utf8_lossy(&res.stdout).trim().to_owned();
            RpcExit::Success {
                value: json!({
                    "repository": {
                        "provider": provider,
                        "nameWithOwner": req.name,
                        "url": url,
                        "sshUrl": Value::Null,
                    },
                    "remoteName": remote_name,
                    "remoteUrl": url,
                    "branch": "main",
                    "status": "pushed",
                }),
            }
        }
        Ok(res) => rpc_fail_obj(json!({
            "_tag": "SourceControlRepositoryError",
            "provider": provider,
            "operation": "publishRepository",
            "detail": String::from_utf8_lossy(&res.stderr).trim(),
        })),
        Err(e) => rpc_fail_obj(json!({
            "_tag": "SourceControlRepositoryError",
            "provider": provider,
            "operation": "publishRepository",
            "detail": e.to_string(),
        })),
    }
}

async fn project_clone_start(state: &AppState, payload: Value) -> RpcExit {
    let project_id = payload
        .get("projectId")
        .and_then(Value::as_str)
        .map_or_else(|| format!("project-{}", Uuid::new_v4()), str::to_owned);
    match source_control_clone(state, payload).await {
        RpcExit::Success { value } => RpcExit::Success {
            value: json!({
                "projectId": project_id,
                "cwd": value["cwd"],
                "remoteUrl": value["remoteUrl"],
                "repository": value["repository"],
            }),
        },
        err @ RpcExit::Failure { .. } => err,
    }
}
