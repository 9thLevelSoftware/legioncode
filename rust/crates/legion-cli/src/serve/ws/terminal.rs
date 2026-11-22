//! Terminal live stream subscriptions (`terminal.attach`, `terminal.observe`, `subscribeTerminalMetadata`).

use std::collections::BTreeMap;
use std::path::PathBuf;

use legion_protocol::rpc_codec::RpcExit;
use legion_server_runtime::TerminalOpenRequest;
use legion_server_runtime::terminal_session::stream_types::{
    TerminalOutputSubscription, TerminalSessionLifecycleStatus, TerminalStreamChunk,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::broadcast::error::RecvError;

use super::stream_ctx::StreamContext;
use crate::serve::ws_mcp::rpc_fail;

/// Returns `true` if `tag` is a terminal streaming RPC.
pub(super) fn handles_tag(tag: &str) -> bool {
    matches!(
        tag,
        "terminal.attach" | "terminal.observe" | "subscribeTerminalMetadata"
    )
}

/// Dispatches `terminal.attach`, `terminal.observe`, or `subscribeTerminalMetadata`.
pub(super) async fn dispatch_stream(ctx: &StreamContext, tag: &str, payload: Value) -> RpcExit {
    match tag {
        "terminal.attach" => stream_terminal_attach(ctx, payload).await,
        "terminal.observe" => stream_terminal_observe(ctx, payload).await,
        "subscribeTerminalMetadata" => stream_terminal_metadata(ctx).await,
        other => rpc_fail(
            "UnsupportedRpcMethod",
            &format!("Unsupported RPC method `{other}`"),
        ),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TerminalAttachPayload {
    thread_id: String,
    terminal_id: String,
    cwd: Option<String>,
    worktree_path: Option<String>,
    cols: Option<u16>,
    rows: Option<u16>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    restart_if_not_running: Option<bool>,
    resume: Option<bool>,
    max_history_bytes: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TerminalObservePayload {
    thread_id: String,
    terminal_id: String,
}

async fn stream_terminal_attach(ctx: &StreamContext, payload: Value) -> RpcExit {
    let req: TerminalAttachPayload = match serde_json::from_value(payload) {
        Ok(parsed) => parsed,
        Err(err) => return rpc_fail("TerminalError", &err.to_string()),
    };

    let existing = ctx.state.terminal_runtime.subscribe_output_with_limit(
        &req.thread_id,
        &req.terminal_id,
        req.max_history_bytes,
    );

    let sub = match existing {
        Ok(sub)
            if !(req.restart_if_not_running == Some(true)
                && matches!(
                    sub.snapshot.status,
                    TerminalSessionLifecycleStatus::Exited | TerminalSessionLifecycleStatus::Error
                )) =>
        {
            sub
        }
        _ => {
            if let Err(exit) = open_for_attach(ctx, &req) {
                return exit;
            }
            match ctx.state.terminal_runtime.subscribe_output_with_limit(
                &req.thread_id,
                &req.terminal_id,
                req.max_history_bytes,
            ) {
                Ok(sub) => sub,
                Err(err) => return rpc_fail("TerminalSessionLookupError", &err.to_string()),
            }
        }
    };

    run_output_subscription(
        ctx,
        &req.thread_id,
        &req.terminal_id,
        req.max_history_bytes,
        sub,
    )
    .await
}

fn open_for_attach(ctx: &StreamContext, req: &TerminalAttachPayload) -> Result<(), RpcExit> {
    let mut open_req = TerminalOpenRequest::default_shell(
        &req.thread_id,
        &req.terminal_id,
        req.cols.unwrap_or(80),
        req.rows.unwrap_or(24),
    );
    if let Some(ref cwd) = req.cwd {
        open_req = open_req.with_cwd(PathBuf::from(cwd));
    }
    if let Some(ref wt) = req.worktree_path {
        open_req = open_req.with_worktree_path(PathBuf::from(wt));
    }
    if let Some(max_bytes) = req.max_history_bytes {
        open_req = open_req.with_max_history_bytes(max_bytes);
    }
    open_req.resume = req.resume.unwrap_or(false);
    open_req.env_overrides = req.env.clone();

    ctx.state
        .terminal_runtime
        .terminal_open(&open_req)
        .map(|_| ())
        .map_err(|err| rpc_fail("TerminalOpenError", &err.to_string()))
}

async fn stream_terminal_observe(ctx: &StreamContext, payload: Value) -> RpcExit {
    let req: TerminalObservePayload = match serde_json::from_value(payload) {
        Ok(parsed) => parsed,
        Err(err) => return rpc_fail("TerminalError", &err.to_string()),
    };
    let sub = match ctx.state.terminal_runtime.subscribe_output_with_limit(
        &req.thread_id,
        &req.terminal_id,
        None,
    ) {
        Ok(sub) => sub,
        Err(err) => return rpc_fail("TerminalSessionLookupError", &err.to_string()),
    };
    run_output_subscription(ctx, &req.thread_id, &req.terminal_id, None, sub).await
}

async fn run_output_subscription(
    ctx: &StreamContext,
    thread_id: &str,
    terminal_id: &str,
    max_history_bytes: Option<usize>,
    mut sub: TerminalOutputSubscription,
) -> RpcExit {
    let mut last_seq = sub.snapshot.sequence;
    let initial = json!({
        "type": "snapshot",
        "snapshot": sub.snapshot,
    });
    if !ctx.send_chunk(initial).await {
        return RpcExit::Success { value: Value::Null };
    }

    loop {
        match sub.receiver.recv().await {
            Ok(chunk) => {
                if chunk.sequence() <= last_seq {
                    continue;
                }
                last_seq = chunk.sequence();
                let is_closed = matches!(chunk, TerminalStreamChunk::Closed { .. });
                if let Some(wire_val) = chunk_to_attach_event(chunk)
                    && !ctx.send_chunk(wire_val).await
                {
                    break;
                }
                if is_closed {
                    break;
                }
            }
            Err(RecvError::Lagged(_)) => {
                let Ok(resync) = ctx.state.terminal_runtime.subscribe_output_with_limit(
                    thread_id,
                    terminal_id,
                    max_history_bytes,
                ) else {
                    break;
                };
                last_seq = resync.snapshot.sequence;
                let snap_val = json!({
                    "type": "snapshot",
                    "snapshot": resync.snapshot,
                });
                sub.receiver = resync.receiver;
                if !ctx.send_chunk(snap_val).await {
                    break;
                }
            }
            Err(RecvError::Closed) => break,
        }
    }

    RpcExit::Success { value: Value::Null }
}

fn chunk_to_attach_event(chunk: TerminalStreamChunk) -> Option<Value> {
    match chunk {
        TerminalStreamChunk::Started {
            thread_id,
            terminal_id,
            sequence,
            snapshot,
        } => Some(json!({
            "type": "restarted",
            "threadId": thread_id,
            "terminalId": terminal_id,
            "sequence": sequence,
            "snapshot": snapshot,
        })),
        TerminalStreamChunk::Output {
            thread_id,
            terminal_id,
            sequence,
            data,
            ..
        } => Some(json!({
            "type": "output",
            "threadId": thread_id,
            "terminalId": terminal_id,
            "sequence": sequence,
            "data": data,
        })),
        other => serde_json::to_value(other).ok(),
    }
}

async fn stream_terminal_metadata(ctx: &StreamContext) -> RpcExit {
    let (snapshot, mut rx) = match ctx.state.terminal_runtime.subscribe_metadata() {
        Ok(ok) => ok,
        Err(err) => return rpc_fail("TerminalError", &err.to_string()),
    };
    let Ok(initial_val) = serde_json::to_value(&snapshot) else {
        return rpc_fail(
            "TerminalError",
            "failed to serialize terminal metadata snapshot",
        );
    };
    if !ctx.send_chunk(initial_val).await {
        return RpcExit::Success { value: Value::Null };
    }

    loop {
        match rx.recv().await {
            Ok(ev) => {
                if let Ok(val) = serde_json::to_value(&ev)
                    && !ctx.send_chunk(val).await
                {
                    break;
                }
            }
            Err(RecvError::Lagged(_)) => {
                let Ok((resync, new_rx)) = ctx.state.terminal_runtime.subscribe_metadata() else {
                    break;
                };
                rx = new_rx;
                if let Ok(val) = serde_json::to_value(&resync)
                    && !ctx.send_chunk(val).await
                {
                    break;
                }
            }
            Err(RecvError::Closed) => break,
        }
    }

    RpcExit::Success { value: Value::Null }
}
