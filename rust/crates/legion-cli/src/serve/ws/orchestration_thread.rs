//! Live `orchestration.subscribeThread` stream subscription (`OrchestrationV2ThreadStreamItem`).

use legion_contracts::id_table::{IdTableTarget, encode_with_id_table};
use legion_contracts::ids::ThreadId;
use legion_orchestrator::projections::{query_wire_thread_bounded, query_wire_thread_detail};
use legion_orchestrator::sink::event_codec::{PersistedEvent, load_thread_events};
use legion_protocol::rpc_codec::{RpcCauseReason, RpcExit};
use serde::Deserialize;
use serde_json::{Value, json};

use super::stream_ctx::StreamContext;
use crate::serve::ws_mcp::rpc_fail;

const MAX_REPLAY_GAP: i64 = 1_000;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubscribeThreadInput {
    thread_id: String,
    after_sequence: Option<i64>,
    accept_bounded_snapshot: Option<bool>,
    accept_compact_ids: Option<bool>,
}

/// Streams initial thread projection snapshot (or delta replay), `synchronized`, and live events.
pub(super) async fn stream_thread(ctx: &StreamContext, payload: Value) -> RpcExit {
    let input: SubscribeThreadInput = match serde_json::from_value(payload) {
        Ok(parsed) => parsed,
        Err(err) => return rpc_fail("ParseError", &err.to_string()),
    };
    let tid = match ThreadId::new(input.thread_id.clone()) {
        Ok(id) => id,
        Err(err) => return thread_projection_fail(&input.thread_id, &err.to_string()),
    };

    let mut sub = ctx.state.orchestrator.sink().pubsub().subscribe();
    let initial_res = {
        let Ok(conn) = ctx.state.orchestrator.sink().db().lock() else {
            return thread_projection_fail(&input.thread_id, "db mutex poisoned");
        };
        build_thread_initial_items(&conn, &tid, &input)
    };
    let (initial_items, mut cursor) = match initial_res {
        Ok(ok) => ok,
        Err(exit) => return exit,
    };

    for item in initial_items {
        if !ctx.send_chunk(item).await {
            return RpcExit::Success { value: Value::Null };
        }
    }

    for ev in sub.drain_ready() {
        if !emit_thread_event(ctx, &ev, &input.thread_id, &mut cursor).await {
            return RpcExit::Success { value: Value::Null };
        }
    }

    while let Some(ev) = sub.recv().await {
        if !emit_thread_event(ctx, &ev, &input.thread_id, &mut cursor).await {
            break;
        }
    }
    RpcExit::Success { value: Value::Null }
}

async fn emit_thread_event(
    ctx: &StreamContext,
    ev: &PersistedEvent,
    thread_id: &str,
    cursor: &mut i64,
) -> bool {
    if ev.event.thread_id.as_str() != thread_id || ev.sequence <= *cursor {
        return true;
    }
    *cursor = ev.sequence;
    let Ok(event_val) = serde_json::to_value(&ev.event) else {
        return true;
    };
    ctx.send_chunk(json!({
        "kind": "event",
        "sequence": ev.sequence,
        "event": event_val,
    }))
    .await
}

fn build_thread_initial_items(
    conn: &rusqlite::Connection,
    tid: &ThreadId,
    input: &SubscribeThreadInput,
) -> Result<(Vec<Value>, i64), RpcExit> {
    let detail = query_wire_thread_detail(conn, tid.as_str())
        .map_err(|err| thread_projection_fail(tid.as_str(), &err.to_string()))?;

    if let Some(after_seq) = input.after_sequence
        && after_seq >= 0
        && after_seq <= detail.snapshot_sequence
        && detail.snapshot_sequence - after_seq <= MAX_REPLAY_GAP
        && let Ok(replayed) = load_thread_events(conn, tid, after_seq)
    {
        let mut out = Vec::with_capacity(replayed.len() + 1);
        for ev in replayed {
            let event_val = serde_json::to_value(&ev.event)
                .map_err(|err| thread_projection_fail(tid.as_str(), &err.to_string()))?;
            out.push(json!({
                "kind": "event",
                "sequence": ev.sequence,
                "event": event_val,
            }));
        }
        out.push(json!({ "kind": "synchronized" }));
        return Ok((out, detail.snapshot_sequence));
    }

    let mut snap_val = if input.accept_bounded_snapshot == Some(true) {
        let bounded = query_wire_thread_bounded(conn, tid.as_str(), None)
            .map_err(|err| thread_projection_fail(tid.as_str(), &err.to_string()))?;
        json!({
            "kind": "snapshot",
            "snapshotSequence": bounded.snapshot_sequence,
            "projection": bounded.projection,
            "historyCursor": bounded.history_cursor,
            "hasMoreHistory": bounded.has_more_history,
            "latestLocalTurnOrdinal": bounded.latest_local_turn_ordinal,
        })
    } else {
        json!({
            "kind": "snapshot",
            "snapshotSequence": detail.snapshot_sequence,
            "projection": detail.projection,
        })
    };

    if input.accept_compact_ids == Some(true)
        && let Some(obj) = snap_val.as_object_mut()
    {
        obj.insert("idTable".to_owned(), Value::Bool(true));
        snap_val = encode_with_id_table(&snap_val, IdTableTarget::Field("projection"))
            .map_err(|err| thread_projection_fail(tid.as_str(), &err.to_string()))?;
    }

    Ok((
        vec![snap_val, json!({ "kind": "synchronized" })],
        detail.snapshot_sequence,
    ))
}

fn thread_projection_fail(thread_id: &str, message: &str) -> RpcExit {
    RpcExit::Failure {
        cause: vec![RpcCauseReason::Fail {
            error: json!({
                "_tag": "OrchestrationV2GetThreadProjectionError",
                "threadId": thread_id,
                "message": message,
            }),
        }],
    }
}
