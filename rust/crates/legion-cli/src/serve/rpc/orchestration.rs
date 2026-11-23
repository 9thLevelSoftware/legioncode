//! Unary orchestration v2 RPC handlers (`orchestration.*` and `orchestration.v2.*`).

use std::sync::Arc;

use legion_orchestrator::projections::{
    WireOlderSettledCursor, WireShellOptions, list_wire_older_settled_threads,
    query_wire_shell_http_snapshot_with_options, query_wire_thread_detail,
};
use legion_protocol::rpc_codec::RpcExit;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{
    AppState, orchestration_commands, orchestration_diff, orchestration_launch,
    orchestration_queries, orchestration_search, parse_optional_payload, parse_payload, rpc_fail,
    rpc_fail_obj, with_db,
};

pub(crate) async fn dispatch(state: &Arc<AppState>, method: &str, payload: Value) -> RpcExit {
    match method {
        "dispatchCommand" => orchestration_commands::dispatch_command(state, payload).await,
        "launchThread" => orchestration_launch::launch_thread(state, payload).await,
        "forceStopThread" => orchestration_launch::force_stop_thread(state, payload).await,
        "stopWorkflow" => orchestration_launch::stop_workflow(state, payload).await,
        "getShellSnapshot" => get_shell_snapshot(state, payload),
        "getArchivedShellSnapshot" => get_archived_shell_snapshot(state, payload),
        "getOlderSettledThreads" => get_older_settled_threads(state, payload),
        "getThreadProjection" => get_thread_projection(state, payload),
        "getThreadFamily" => orchestration_queries::get_thread_family(state, payload),
        "getThreadTitles" => orchestration_queries::get_thread_titles(state, payload),
        "getThreadLinkMetadata" => orchestration_queries::get_thread_link_metadata(state, payload),
        "getWorktreeThreadIds" => orchestration_queries::get_worktree_thread_ids(state, payload),
        "getTurnItem" => orchestration_diff::get_turn_item(state, payload),
        "getWorkflowScript" => orchestration_diff::get_workflow_script(state, payload),
        "getNativeChildWork" => orchestration_diff::get_native_child_work(payload),
        "getTurnDiff" => orchestration_diff::get_turn_diff(state, payload).await,
        "getFullThreadDiff" => orchestration_diff::get_full_thread_diff(state, payload).await,
        "searchThreads" => orchestration_search::search_threads(state, payload),
        "searchThread" => orchestration_queries::search_thread(state, payload),
        _ => rpc_fail(
            "UnsupportedRpcMethod",
            &format!("Unsupported RPC method `orchestration.{method}`"),
        ),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ShellSnapshotPayload {
    omit_idle_subagents: Option<bool>,
    omit_older_settled: Option<bool>,
}

fn get_shell_snapshot(state: &AppState, payload: Value) -> RpcExit {
    let req: ShellSnapshotPayload = match parse_optional_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let options = WireShellOptions {
        omit_idle_subagents: req.omit_idle_subagents.unwrap_or(true),
        omit_older_settled: req.omit_older_settled.unwrap_or(true),
    };
    let env_id = state.descriptor.environment_id.as_str();
    with_db(state, |conn| {
        query_wire_shell_http_snapshot_with_options(conn, env_id, options)
            .map(|snap| RpcExit::Success { value: json!(snap) })
            .map_err(|err| shell_err(&err.to_string()))
    })
    .unwrap_or_else(|e| e)
}

fn get_archived_shell_snapshot(state: &AppState, payload: Value) -> RpcExit {
    let _: Value = match parse_optional_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let env_id = state.descriptor.environment_id.as_str();
    with_db(state, |conn| {
        let full = query_wire_shell_http_snapshot_with_options(
            conn,
            env_id,
            WireShellOptions {
                omit_idle_subagents: false,
                omit_older_settled: false,
            },
        )
        .map_err(|err| shell_err(&err.to_string()))?;
        Ok(RpcExit::Success {
            value: json!({
                "schemaVersion": full.schema_version,
                "snapshotSequence": full.snapshot_sequence,
                "projects": full.projects,
                "threads": full.archived_threads,
            }),
        })
    })
    .unwrap_or_else(|e| e)
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct OlderSettledPayload {
    #[serde(alias = "cursor")]
    before: Option<WireOlderSettledCursor>,
    project_id: Option<String>,
    project_ids: Option<Vec<String>>,
    limit: Option<usize>,
}

fn get_older_settled_threads(state: &AppState, payload: Value) -> RpcExit {
    let req: OlderSettledPayload = match parse_optional_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let single_project = req.project_id.as_deref().or_else(|| {
        req.project_ids
            .as_deref()
            .filter(|ids| ids.len() == 1)
            .and_then(|ids| ids.first().map(String::as_str))
    });
    with_db(state, |conn| {
        let mut page = list_wire_older_settled_threads(
            conn,
            single_project,
            req.before.as_ref(),
            req.limit.unwrap_or(50),
        )
        .map_err(|err| shell_err(&err.to_string()))?;
        if let Some(ref ids) = req.project_ids
            && ids.len() > 1
        {
            page.threads
                .retain(|t| ids.iter().any(|p| p == &t.project_id));
            page.counts
                .retain(|c| ids.iter().any(|p| p == &c.project_id));
        }
        Ok(RpcExit::Success { value: json!(page) })
    })
    .unwrap_or_else(|e| e)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadIdPayload {
    pub(crate) thread_id: String,
}

fn get_thread_projection(state: &AppState, payload: Value) -> RpcExit {
    let req: ThreadIdPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    with_db(state, |conn| {
        let detail = query_wire_thread_detail(conn, &req.thread_id).map_err(|err| {
            rpc_fail_obj(json!({
                "_tag": "OrchestrationV2GetThreadProjectionError",
                "threadId": req.thread_id,
                "message": err.to_string(),
            }))
        })?;
        Ok(RpcExit::Success {
            value: json!(detail.projection),
        })
    })
    .unwrap_or_else(|e| e)
}

pub(crate) fn shell_err(msg: &str) -> RpcExit {
    rpc_fail_obj(json!({
        "_tag": "OrchestrationV2GetShellSnapshotError",
        "message": msg,
    }))
}
