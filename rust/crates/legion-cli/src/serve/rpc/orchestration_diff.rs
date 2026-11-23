//! Turn/full-thread checkpoint diff, turn item, workflow script, and native child work handlers.

use std::fs;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;

use legion_contracts::ids::ThreadId;
use legion_protocol::rpc_codec::RpcExit;
use legion_scm::checkpoints::checkpoint_ref_for_turn;
use legion_scm::checkpoints::store::CheckpointStore;
use rusqlite::{OptionalExtension as _, params};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::process::Command;

use super::{AppState, parse_payload, rpc_fail_obj, with_db};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TurnDiffPayload {
    thread_id: String,
    from_turn_count: u32,
    to_turn_count: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FullThreadDiffPayload {
    thread_id: String,
    to_turn_count: u32,
}

pub(crate) async fn get_turn_diff(state: &Arc<AppState>, payload: Value) -> RpcExit {
    let req: TurnDiffPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    query_diff_range(
        state,
        &req.thread_id,
        req.from_turn_count,
        req.to_turn_count,
        false,
    )
    .await
}

pub(crate) async fn get_full_thread_diff(state: &Arc<AppState>, payload: Value) -> RpcExit {
    let req: FullThreadDiffPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    query_diff_range(state, &req.thread_id, 0, req.to_turn_count, true).await
}

async fn query_diff_range(
    state: &Arc<AppState>,
    thread_id_raw: &str,
    from_turn: u32,
    to_turn: u32,
    is_full: bool,
) -> RpcExit {
    let err_tag = if is_full {
        "OrchestrationGetFullThreadDiffError"
    } else {
        "OrchestrationGetTurnDiffError"
    };
    if from_turn > to_turn {
        return rpc_fail_obj(json!({
            "_tag": err_tag,
            "message": format!("fromTurnCount ({from_turn}) exceeds toTurnCount ({to_turn})"),
        }));
    }
    if from_turn == to_turn {
        return RpcExit::Success {
            value: json!({
                "threadId": thread_id_raw,
                "fromTurnCount": from_turn,
                "toTurnCount": to_turn,
                "diff": "",
            }),
        };
    }

    let thread_id = match ThreadId::new(thread_id_raw) {
        Ok(id) => id,
        Err(err) => {
            return rpc_fail_obj(json!({
                "_tag": err_tag,
                "message": err.to_string(),
            }));
        }
    };

    let stored = with_db(state, |conn| {
        let store = CheckpointStore::new(conn);
        if let Ok(Some(blob)) = store.get_diff_blob(&thread_id, from_turn, to_turn) {
            return Ok((Some(blob.diff), None));
        }
        let workspace: Option<String> = conn
            .query_row(
                "SELECT COALESCE(
                   NULLIF(t.worktree_path, ''),
                   NULLIF(json_extract(t.payload_json, '$.worktreePath'), ''),
                   p.workspace_root
                 )
                 FROM orchestration_v2_projection_threads AS t
                 LEFT JOIN projection_projects AS p ON p.project_id = t.project_id
                 WHERE t.thread_id = ?1",
                params![thread_id.as_str()],
                |r| r.get(0),
            )
            .optional()
            .unwrap_or(None)
            .flatten();
        Ok((None, workspace))
    });

    match stored {
        Ok((Some(diff), _)) => RpcExit::Success {
            value: json!({
                "threadId": thread_id.as_str(),
                "fromTurnCount": from_turn,
                "toTurnCount": to_turn,
                "diff": diff,
            }),
        },
        Ok((None, Some(cwd))) => {
            let from_ref = checkpoint_ref_for_turn(&thread_id, from_turn)
                .ok()
                .map(|r| r.as_str().to_owned());
            let to_ref = checkpoint_ref_for_turn(&thread_id, to_turn)
                .ok()
                .map(|r| r.as_str().to_owned());
            if let (Some(r1), Some(r2)) = (from_ref, to_ref)
                && let Some(diff) = run_git_checkpoint_diff(Path::new(&cwd), &r1, &r2).await
            {
                return RpcExit::Success {
                    value: json!({
                        "threadId": thread_id.as_str(),
                        "fromTurnCount": from_turn,
                        "toTurnCount": to_turn,
                        "diff": diff,
                    }),
                };
            }
            rpc_fail_obj(json!({
                "_tag": err_tag,
                "message": format!("Checkpoint diff unavailable for `{}` ({from_turn}..{to_turn}).", thread_id.as_str()),
            }))
        }
        Ok((None, None)) => rpc_fail_obj(json!({
            "_tag": err_tag,
            "message": format!("Thread `{}` checkpoint diff unavailable.", thread_id.as_str()),
        })),
        Err(e) => e,
    }
}

const MAX_WORKFLOW_SCRIPT_BYTES: usize = 256 * 1024;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetTurnItemPayload {
    thread_id: String,
    item_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkflowScriptPayload {
    script_path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NativeChildWorkPayload {
    thread_id: String,
}

pub(crate) fn get_turn_item(state: &AppState, payload: Value) -> RpcExit {
    let req: GetTurnItemPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    with_db(state, |conn| {
        let raw: Option<String> = conn
            .query_row(
                "SELECT payload_json FROM orchestration_v2_projection_turn_items
                 WHERE thread_id = ?1 AND turn_item_id = ?2",
                params![req.thread_id, req.item_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|err| {
                rpc_fail_obj(json!({
                    "_tag": "OrchestrationV2GetThreadProjectionError",
                    "threadId": req.thread_id,
                    "message": err.to_string(),
                }))
            })?;
        let item: Value = raw.map_or(Value::Null, |txt| {
            serde_json::from_str(&txt).unwrap_or(Value::Null)
        });
        Ok(RpcExit::Success {
            value: json!({ "item": item }),
        })
    })
    .unwrap_or_else(|e| e)
}

pub(crate) fn get_workflow_script(state: &AppState, payload: Value) -> RpcExit {
    let req: WorkflowScriptPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let path = Path::new(&req.script_path);
    if !path.is_absolute() {
        return rpc_fail_obj(json!({
            "_tag": "OrchestrationGetWorkflowScriptError",
            "reason": "invalid-path",
            "scriptPath": req.script_path,
            "message": "Workflow scripts must be absolute .js paths.",
        }));
    }
    if path.extension().and_then(|e| e.to_str()) != Some("js") {
        return rpc_fail_obj(json!({
            "_tag": "OrchestrationGetWorkflowScriptError",
            "reason": "not-js",
            "scriptPath": req.script_path,
            "message": "Workflow scripts must be absolute .js paths.",
        }));
    }
    let workflows_root = state.paths.state_dir.join("workflows");
    let Ok(canonical_root) = fs::canonicalize(&workflows_root) else {
        return rpc_fail_obj(json!({
            "_tag": "OrchestrationGetWorkflowScriptError",
            "reason": "root-unavailable",
            "scriptPath": req.script_path,
            "message": "Script root unavailable.",
        }));
    };
    let Ok(canonical_file) = fs::canonicalize(path) else {
        return rpc_fail_obj(json!({
            "_tag": "OrchestrationGetWorkflowScriptError",
            "reason": "not-found",
            "scriptPath": req.script_path,
            "message": "Script not found.",
        }));
    };
    if !canonical_file.starts_with(&canonical_root) {
        return rpc_fail_obj(json!({
            "_tag": "OrchestrationGetWorkflowScriptError",
            "reason": "outside-root",
            "scriptPath": req.script_path,
            "message": "Script path is outside the workflow scripts root.",
        }));
    }
    let Ok(bytes) = fs::read(&canonical_file) else {
        return rpc_fail_obj(json!({
            "_tag": "OrchestrationGetWorkflowScriptError",
            "reason": "read-failed",
            "scriptPath": req.script_path,
            "message": "Script read failed.",
        }));
    };
    let truncated = bytes.len() > MAX_WORKFLOW_SCRIPT_BYTES;
    let slice = &bytes[..bytes.len().min(MAX_WORKFLOW_SCRIPT_BYTES)];
    let contents = String::from_utf8_lossy(slice).into_owned();
    RpcExit::Success {
        value: json!({
            "scriptPath": canonical_file.to_string_lossy(),
            "contents": contents,
            "truncated": truncated,
        }),
    }
}

pub(crate) fn get_native_child_work(payload: Value) -> RpcExit {
    let req: NativeChildWorkPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let _ = &req.thread_id;
    RpcExit::Success {
        value: json!({ "children": [] }),
    }
}

async fn run_git_checkpoint_diff(cwd: &Path, from_ref: &str, to_ref: &str) -> Option<String> {
    if !cwd.is_dir() {
        return None;
    }
    let out = Command::new("git")
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(["diff", "--no-color", "--unified=3", from_ref, to_ref])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}
