//! Streaming WS RPCs for workflows (`workflows.subscribe`, `workflows.counts`, `#38882`)
//! and linked-environment threads (`peerLinks.subscribeThread`, `#39406`).

use legion_contracts::peer_links::PeerLinkThreadInput;
use legion_protocol::rpc_codec::RpcExit;
use legion_server_runtime::relay_peer_thread_stream;
use serde_json::{Value, json};
use tokio::sync::broadcast::error::RecvError;

use super::super::stream_ctx::StreamContext;
use crate::serve::rpc::peer_links_rpc::map_thread_read_err;
use crate::serve::ws_mcp::rpc_fail;

fn empty_workflows_snapshot() -> Value {
    json!({
        "workflows": [],
        "runs": [],
        "questions": [],
        "definition": { "revision": 0, "problem": null },
    })
}

fn empty_workflow_counts() -> Value {
    json!({ "needsYou": 0, "active": 0, "total": 0 })
}

pub(super) async fn stream_workflows_snapshot(ctx: &StreamContext) -> RpcExit {
    let Some(ref mgr) = ctx.state.runtime_http.workflow_manager else {
        if ctx.send_chunk(empty_workflows_snapshot()).await {
            std::future::pending::<()>().await;
        }
        return RpcExit::Success { value: Value::Null };
    };

    let Ok(initial) = mgr.snapshot() else {
        return rpc_fail("WorkflowRequestError", "Failed to read workflow snapshot.");
    };
    if !ctx.send_chunk(json!(initial)).await {
        return RpcExit::Success { value: Value::Null };
    }

    let Some(mut rx) = ctx
        .state
        .workflow_store
        .as_ref()
        .map(legion_workflows::store::WorkflowStore::subscribe_changes)
    else {
        std::future::pending::<()>().await;
        return RpcExit::Success { value: Value::Null };
    };

    while let Ok(_) | Err(RecvError::Lagged(_)) = rx.recv().await {
        if let Ok(snap) = mgr.snapshot()
            && !ctx.send_chunk(json!(snap)).await
        {
            break;
        }
    }
    RpcExit::Success { value: Value::Null }
}

pub(super) async fn stream_workflows_counts(ctx: &StreamContext) -> RpcExit {
    let Some(engine) = ctx.state.runtime_http.workflow_engine() else {
        if ctx.send_chunk(empty_workflow_counts()).await {
            std::future::pending::<()>().await;
        }
        return RpcExit::Success { value: Value::Null };
    };

    let Ok(initial) = engine.counts() else {
        return rpc_fail("WorkflowRequestError", "Failed to read workflow counts.");
    };
    if !ctx.send_chunk(json!(initial)).await {
        return RpcExit::Success { value: Value::Null };
    }

    let Some(mut rx) = ctx
        .state
        .workflow_store
        .as_ref()
        .map(legion_workflows::store::WorkflowStore::subscribe_changes)
    else {
        std::future::pending::<()>().await;
        return RpcExit::Success { value: Value::Null };
    };

    while let Ok(_) | Err(RecvError::Lagged(_)) = rx.recv().await {
        if let Ok(counts) = engine.counts()
            && !ctx.send_chunk(json!(counts)).await
        {
            break;
        }
    }
    RpcExit::Success { value: Value::Null }
}

pub(super) async fn stream_peer_thread(ctx: &StreamContext, payload: Value) -> RpcExit {
    let req: PeerLinkThreadInput = match serde_json::from_value(payload) {
        Ok(p) => p,
        Err(err) => return rpc_fail("ParseError", &format!("Invalid payload: {err}")),
    };
    let events = match relay_peer_thread_stream(
        &ctx.state.runtime_http,
        &req.environment_id,
        &req.thread_id,
        &ctx.state.descriptor,
    )
    .await
    {
        Ok(list) => list,
        Err(err) => return map_thread_read_err(&err),
    };
    for ev in events {
        if !ctx.send_chunk(ev).await {
            return RpcExit::Success { value: Value::Null };
        }
    }
    std::future::pending::<()>().await;
    RpcExit::Success { value: Value::Null }
}

#[cfg(test)]
mod tests {
    use legion_contracts::workflows::{WorkflowCounts, WorkflowRunsSnapshot};

    use super::{empty_workflow_counts, empty_workflows_snapshot};

    #[test]
    fn workflow_fallbacks_deserialize_strict_contracts() -> Result<(), serde_json::Error> {
        let snap: WorkflowRunsSnapshot = serde_json::from_value(empty_workflows_snapshot())?;
        assert_eq!(snap.workflows, []);
        assert_eq!(snap.runs, []);
        assert_eq!(snap.questions, []);
        assert!(
            snap.definition
                .is_some_and(|d| d.revision == 0 && d.problem.is_none())
        );

        let counts: WorkflowCounts = serde_json::from_value(empty_workflow_counts())?;
        assert_eq!(counts, WorkflowCounts::default());
        Ok(())
    }
}
