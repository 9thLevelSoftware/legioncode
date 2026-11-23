//! Live orchestration WebSocket stream subscriptions (`subscribeShell`, `subscribeThread`).

use legion_orchestrator::projections::{
    WireShellOptions, WireShellSnapshot, WireShellStreamEvent, build_wire_shell_patch_from_events,
    query_wire_shell_http_snapshot_with_options,
};
use legion_protocol::rpc_codec::RpcExit;
use serde::Deserialize;
use serde_json::Value;

use super::orchestration_thread::stream_thread;
use super::stream_ctx::StreamContext;
use crate::serve::ws_mcp::rpc_fail;

/// Returns `true` if `tag` is a live orchestration pubsub streaming RPC.
pub(super) fn handles_tag(tag: &str) -> bool {
    matches!(
        tag,
        "orchestration.subscribeShell"
            | "orchestration.v2.subscribeShell"
            | "orchestration.subscribeThread"
            | "orchestration.v2.subscribeThread"
    )
}

/// Dispatches an orchestration streaming RPC until completion, interruption, or socket close.
pub(super) async fn dispatch_stream(ctx: &StreamContext, tag: &str, payload: Value) -> RpcExit {
    match tag {
        "orchestration.subscribeShell" | "orchestration.v2.subscribeShell" => {
            stream_shell(ctx, payload).await
        }
        "orchestration.subscribeThread" | "orchestration.v2.subscribeThread" => {
            stream_thread(ctx, payload).await
        }
        other => rpc_fail(
            "UnsupportedRpcMethod",
            &format!("Unsupported RPC method `{other}`"),
        ),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubscribeShellInput {
    after_sequence: Option<i64>,
    http_snapshot_fallback: Option<bool>,
    omit_idle_subagents: Option<bool>,
    omit_older_settled: Option<bool>,
}

async fn stream_shell(ctx: &StreamContext, payload: Value) -> RpcExit {
    let input: SubscribeShellInput = match serde_json::from_value(payload) {
        Ok(parsed) => parsed,
        Err(err) => return rpc_fail("OrchestrationV2GetShellSnapshotError", &err.to_string()),
    };
    let options = WireShellOptions {
        omit_idle_subagents: input.omit_idle_subagents.unwrap_or(true),
        omit_older_settled: input.omit_older_settled.unwrap_or(true),
    };
    let env_id = ctx.state.descriptor.environment_id.as_str();
    let mut sub = ctx.state.orchestrator.sink().pubsub().subscribe();
    let mut seq_rx = ctx.state.orchestrator.sink().pubsub().subscribe_sequences();

    let (initial_items, mut cursor, halt_after_emit) =
        match load_initial_shell_items(ctx, &input, env_id, options) {
            Ok(loaded) => loaded,
            Err(exit) => return exit,
        };

    for item in initial_items {
        let Ok(val) = serde_json::to_value(&item) else {
            return rpc_fail(
                "OrchestrationV2GetShellSnapshotError",
                "failed to serialize shell event",
            );
        };
        if !ctx.send_chunk(val).await {
            return RpcExit::Success { value: Value::Null };
        }
    }
    if halt_after_emit {
        return RpcExit::Success { value: Value::Null };
    }

    let ready = sub.drain_ready();
    let max_ready = ready
        .iter()
        .map(|ev| ev.sequence)
        .fold(*seq_rx.borrow_and_update(), i64::max);
    if max_ready > cursor && !emit_shell_patch(ctx, &mut cursor, max_ready, env_id).await {
        return RpcExit::Success { value: Value::Null };
    }

    loop {
        let max_seq = tokio::select! {
            ev = sub.recv() => {
                let Some(ev) = ev else { break; };
                sub.drain_ready()
                    .into_iter()
                    .fold(ev.sequence, |acc, extra| acc.max(extra.sequence))
            }
            changed = seq_rx.changed() => {
                if changed.is_err() {
                    break;
                }
                *seq_rx.borrow_and_update()
            }
        };
        if max_seq <= cursor {
            continue;
        }
        if !emit_shell_patch(ctx, &mut cursor, max_seq, env_id).await {
            break;
        }
    }
    RpcExit::Success { value: Value::Null }
}

fn load_initial_shell_items(
    ctx: &StreamContext,
    input: &SubscribeShellInput,
    env_id: &str,
    options: WireShellOptions,
) -> Result<(Vec<WireShellStreamEvent>, i64, bool), RpcExit> {
    let conn = ctx
        .state
        .orchestrator
        .sink()
        .db()
        .lock()
        .map_err(|_| rpc_fail("OrchestrationV2GetShellSnapshotError", "db mutex poisoned"))?;
    let Some(after_seq) = input.after_sequence else {
        let (snap_ev, seq) = build_full_shell_snapshot_item(&conn, env_id, options)
            .map_err(|err| rpc_fail("OrchestrationV2GetShellSnapshotError", &err))?;
        return Ok((
            vec![snap_ev, WireShellStreamEvent::Synchronized],
            seq,
            false,
        ));
    };
    let patch = build_wire_shell_patch_from_events(&conn, after_seq, env_id, None)
        .map_err(|err| rpc_fail("OrchestrationV2GetShellSnapshotError", &err.to_string()))?;
    if matches!(patch.as_slice(), [WireShellStreamEvent::SnapshotRequired]) {
        if input.http_snapshot_fallback == Some(true) {
            return Ok((
                vec![WireShellStreamEvent::SnapshotRequired],
                after_seq,
                true,
            ));
        }
        let (snap_ev, seq) = build_full_shell_snapshot_item(&conn, env_id, options)
            .map_err(|err| rpc_fail("OrchestrationV2GetShellSnapshotError", &err))?;
        return Ok((
            vec![snap_ev, WireShellStreamEvent::Synchronized],
            seq,
            false,
        ));
    }
    let max_seq = patch
        .iter()
        .filter_map(shell_event_sequence)
        .max()
        .unwrap_or(after_seq);
    let mut items = patch;
    items.push(WireShellStreamEvent::Synchronized);
    Ok((items, max_seq, false))
}

fn build_full_shell_snapshot_item(
    conn: &rusqlite::Connection,
    env_id: &str,
    options: WireShellOptions,
) -> Result<(WireShellStreamEvent, i64), String> {
    let http = query_wire_shell_http_snapshot_with_options(conn, env_id, options)
        .map_err(|err| err.to_string())?;
    let seq = http.snapshot_sequence;
    let ev = WireShellStreamEvent::Snapshot {
        snapshot: Box::new(WireShellSnapshot {
            schema_version: http.schema_version,
            snapshot_sequence: http.snapshot_sequence,
            threads: http.threads,
            archived_threads: http.archived_threads,
            projects: http.projects,
            older_settled: http.older_settled,
        }),
        resolved_repository_identity_roots: None,
    };
    Ok((ev, seq))
}

async fn emit_shell_patch(
    ctx: &StreamContext,
    cursor: &mut i64,
    max_seq: i64,
    env_id: &str,
) -> bool {
    let patch_res = {
        let Ok(conn) = ctx.state.orchestrator.sink().db().lock() else {
            return false;
        };
        build_wire_shell_patch_from_events(&conn, *cursor, env_id, None)
    };
    let Ok(patch) = patch_res else {
        return false;
    };
    *cursor = (*cursor).max(max_seq);
    if patch.is_empty() {
        return true;
    }
    let mut values = Vec::with_capacity(patch.len());
    for item in patch {
        if let Ok(val) = serde_json::to_value(&item) {
            values.push(val);
        }
    }
    ctx.send_chunks(values).await
}

const fn shell_event_sequence(ev: &WireShellStreamEvent) -> Option<i64> {
    match ev {
        WireShellStreamEvent::ProjectUpdated { sequence, .. }
        | WireShellStreamEvent::ProjectRemoved { sequence, .. }
        | WireShellStreamEvent::ThreadUpdated { sequence, .. }
        | WireShellStreamEvent::ThreadRemoved { sequence, .. } => Some(*sequence),
        WireShellStreamEvent::Synchronized
        | WireShellStreamEvent::SnapshotRequired
        | WireShellStreamEvent::Snapshot { .. } => None,
    }
}
