//! Auxiliary streaming RPCs: scheduled tasks, device/provider setup, and maintenance.

use legion_orchestrator::time::now_iso_utc;
use legion_protocol::rpc_codec::RpcExit;
use serde::Deserialize;
use serde_json::{Value, json};

#[path = "other_streams_helpers.rs"]
mod helpers;

use self::helpers::{load_scheduled_tasks_json, stream_git_stacked_action};
use super::stream_ctx::StreamContext;
use super::telemetry_vcs::{stream_resource_telemetry, stream_vcs_branch, stream_vcs_status};
use crate::serve::ws_mcp::rpc_fail;

/// Returns `true` for all auxiliary `stream: true` WS RPC methods.
pub(super) fn handles_tag(tag: &str) -> bool {
    matches!(
        tag,
        "subscribeVcsStatus"
            | "subscribeVcsBranch"
            | "subscribeResourceTelemetry"
            | "scheduledTasks.subscribe"
            | "subscribeProjectClones"
            | "subscribeWorktreeSetup"
            | "subscribePreviewEvents"
            | "subscribeDiscoveredLocalServers"
            | "subscribeDeviceState"
            | "provider.auth.subscribe"
            | "provider.install.subscribe"
            | "provider.chatgpt.handoff.subscribe"
            | "pullRequests.subscribeRefreshes"
            | "pullRequests.subscribeLinkedThreadRefreshes"
            | "server.getStorageCleanupReport"
            | "server.getDatabaseStorage"
            | "server.updateServerWithProgress"
            | "cloud.installRelayClient"
            | "git.runStackedAction"
            | "orchestration.subscribeCommandOutput"
            | "orchestration.v2.subscribeCommandOutput"
            | "orchestration.subscribePromptSuggestion"
            | "orchestration.v2.subscribePromptSuggestion"
            | "orchestration.searchThreadStream"
            | "orchestration.v2.searchThreadStream"
    )
}

/// Dispatches an auxiliary streaming RPC and emits its initial contract-valid payload.
pub(super) async fn dispatch_stream(ctx: &StreamContext, tag: &str, payload: Value) -> RpcExit {
    match tag {
        "subscribeVcsStatus" => stream_vcs_status(ctx, payload).await,
        "subscribeVcsBranch" => stream_vcs_branch(ctx, payload).await,
        "subscribeResourceTelemetry" => stream_resource_telemetry(ctx).await,
        "scheduledTasks.subscribe" => stream_scheduled_tasks(ctx).await,
        "subscribeProjectClones" => stay_open_with(ctx, json!([])).await,
        "subscribeWorktreeSetup" => stay_open_with(ctx, Value::Null).await,
        "subscribePreviewEvents" => stay_open_quiet().await,
        "subscribeDiscoveredLocalServers" => {
            let val = json!({
                "servers": [],
                "scannedAt": now_iso_utc(),
                "configuredUrlProbing": true,
            });
            stay_open_with(ctx, val).await
        }
        "subscribeDeviceState" => {
            let val = crate::serve::rpc::preview_device_peers::device_state_snapshot(&ctx.state);
            stay_open_with(ctx, val).await
        }
        "provider.auth.subscribe" => stream_provider_auth(ctx, payload).await,
        "provider.install.subscribe" => stream_provider_install(ctx, payload).await,
        "provider.chatgpt.handoff.subscribe" => stream_chatgpt_handoff(ctx, payload).await,
        "pullRequests.subscribeRefreshes" | "pullRequests.subscribeLinkedThreadRefreshes" => {
            stay_open_with(ctx, json!(0)).await
        }
        "server.getStorageCleanupReport" => {
            let _ = ctx.send_chunk(Value::Null).await;
            RpcExit::Success { value: Value::Null }
        }
        "server.getDatabaseStorage" => stream_database_storage(ctx).await,
        "server.updateServerWithProgress" => {
            let _ = ctx
                .send_chunk(json!({
                    "type": "complete",
                    "result": {
                        "targetVersion": env!("CARGO_PKG_VERSION"),
                        "method": "respawn",
                    },
                }))
                .await;
            RpcExit::Success { value: Value::Null }
        }
        "cloud.installRelayClient" => {
            let _ = ctx
                .send_chunk(json!({
                    "type": "complete",
                    "status": {
                        "status": "missing",
                        "version": env!("CARGO_PKG_VERSION"),
                    },
                }))
                .await;
            RpcExit::Success { value: Value::Null }
        }
        "git.runStackedAction" => stream_git_stacked_action(ctx, payload).await,
        "orchestration.subscribeCommandOutput" | "orchestration.v2.subscribeCommandOutput" => {
            stream_command_output(ctx, payload).await
        }
        "orchestration.subscribePromptSuggestion"
        | "orchestration.v2.subscribePromptSuggestion" => {
            stream_prompt_suggestion(ctx, payload).await
        }
        "orchestration.searchThreadStream" | "orchestration.v2.searchThreadStream" => {
            stream_search_thread(ctx).await
        }
        other => rpc_fail(
            "UnsupportedRpcMethod",
            &format!("Unsupported RPC method `{other}`"),
        ),
    }
}

async fn stream_command_output(ctx: &StreamContext, payload: Value) -> RpcExit {
    let valid = payload.get("threadId").and_then(Value::as_str).is_some()
        && payload.get("itemId").and_then(Value::as_str).is_some();
    if !valid {
        return rpc_fail(
            "OrchestrationV2CommandOutputError",
            "missing threadId or itemId",
        );
    }
    let _ = ctx
        .send_chunk(json!({
            "kind": "replace",
            "text": "",
            "truncated": false,
            "running": false,
        }))
        .await;
    RpcExit::Success { value: Value::Null }
}

async fn stream_prompt_suggestion(ctx: &StreamContext, payload: Value) -> RpcExit {
    if payload.get("threadId").and_then(Value::as_str).is_none() {
        return rpc_fail("ParseError", "missing threadId");
    }
    stay_open_with(ctx, Value::Null).await
}

async fn stream_search_thread(ctx: &StreamContext) -> RpcExit {
    let _ = ctx
        .send_chunk(json!({
            "snapshotSequence": 0,
            "totalMatches": 0,
            "activeIndex": 0,
            "match": null,
        }))
        .await;
    RpcExit::Success { value: Value::Null }
}

async fn stay_open_with(ctx: &StreamContext, value: Value) -> RpcExit {
    if ctx.send_chunk(value).await {
        std::future::pending::<()>().await;
    }
    RpcExit::Success { value: Value::Null }
}

async fn stay_open_quiet() -> RpcExit {
    std::future::pending::<()>().await;
    RpcExit::Success { value: Value::Null }
}

async fn stream_scheduled_tasks(ctx: &StreamContext) -> RpcExit {
    let tasks_res: Result<Vec<Value>, String> = {
        let Ok(conn) = ctx.state.orchestrator.sink().db().lock() else {
            return rpc_fail("ScheduledTaskError", "db mutex poisoned");
        };
        load_scheduled_tasks_json(&conn).map_err(|err| err.to_string())
    };
    let tasks = match tasks_res {
        Ok(list) => list,
        Err(msg) => return rpc_fail("ScheduledTaskError", &msg),
    };
    stay_open_with(ctx, json!({ "tasks": tasks, "timeZone": "UTC" })).await
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InstanceInput {
    #[serde(default)]
    instance_id: Option<String>,
}

async fn stream_provider_auth(ctx: &StreamContext, payload: Value) -> RpcExit {
    let instance_id = serde_json::from_value::<InstanceInput>(payload)
        .ok()
        .and_then(|i| i.instance_id)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "codex".to_owned());
    let val = idle_provider_auth_json(&instance_id);
    stay_open_with(ctx, val).await
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DriverInput {
    #[serde(default)]
    driver: Option<String>,
}

async fn stream_provider_install(ctx: &StreamContext, payload: Value) -> RpcExit {
    let driver = serde_json::from_value::<DriverInput>(payload)
        .ok()
        .and_then(|i| i.driver)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "codex".to_owned());
    let val = json!({
        "driver": driver,
        "operationId": null,
        "phase": "idle",
        "downloadedBytes": 0,
        "totalBytes": null,
        "version": null,
        "installedVersion": null,
        "canRemove": false,
        "message": null,
    });
    stay_open_with(ctx, val).await
}

async fn stream_chatgpt_handoff(ctx: &StreamContext, payload: Value) -> RpcExit {
    let instance_id = serde_json::from_value::<InstanceInput>(payload)
        .ok()
        .and_then(|i| i.instance_id)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "codex".to_owned());
    let val = json!({
        "phase": "auth",
        "state": idle_provider_auth_json(&instance_id),
    });
    stay_open_with(ctx, val).await
}

fn idle_provider_auth_json(instance_id: &str) -> Value {
    json!({
        "instanceId": instance_id,
        "phase": "idle",
        "flowId": null,
        "authorizationUrl": null,
        "expiresAt": null,
        "message": null,
    })
}

async fn stream_database_storage(ctx: &StreamContext) -> RpcExit {
    let db_bytes = std::fs::metadata(&ctx.state.paths.db_path).map_or(0_u64, |m| m.len());
    let (reclaimable_bytes, incremental_vacuum) = {
        let Ok(conn) = ctx.state.orchestrator.sink().db().lock() else {
            return rpc_fail("DatabaseReclaimError", "db mutex poisoned");
        };
        let auto_vacuum: i64 = conn
            .query_row("PRAGMA auto_vacuum", [], |row| row.get(0))
            .unwrap_or(0);
        let freelist: i64 = conn
            .query_row("PRAGMA freelist_count", [], |row| row.get(0))
            .unwrap_or(0);
        let page_size: i64 = conn
            .query_row("PRAGMA page_size", [], |row| row.get(0))
            .unwrap_or(4096);
        let reclaimable = freelist.max(0).saturating_mul(page_size.max(0));
        (reclaimable, auto_vacuum == 2)
    };

    let status = json!({
        "databaseBytes": db_bytes,
        "reclaimableBytes": reclaimable_bytes,
        "incrementalVacuumEnabled": incremental_vacuum,
        "compactCommand": "legioncode doctor",
    });
    let _ = ctx.send_chunk(status).await;
    RpcExit::Success { value: Value::Null }
}
