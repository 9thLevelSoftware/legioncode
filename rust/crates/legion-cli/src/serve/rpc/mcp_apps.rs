//! MCP Apps RPC handlers (`mcpApps.*`).

use legion_protocol::rpc_codec::RpcExit;
use rusqlite::params;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{AppState, parse_payload, rpc_fail, rpc_fail_obj, utc_now_iso, with_db};

pub(crate) fn dispatch(state: &AppState, tag: &str, payload: Value) -> RpcExit {
    match tag {
        "mcpApps.callTool" => mcp_call_tool(&payload),
        "mcpApps.toolInfo" => mcp_tool_info(&payload),
        "mcpApps.readResource" => mcp_read_resource(&payload),
        "mcpApps.updateModelContext" => mcp_update_model_context(state, payload),
        _ => rpc_fail(
            "UnsupportedRpcMethod",
            &format!("Unsupported RPC method `{tag}`"),
        ),
    }
}

fn mcp_call_tool(payload: &Value) -> RpcExit {
    let thread_id = payload
        .get("threadId")
        .and_then(Value::as_str)
        .unwrap_or("thread-default");
    rpc_fail_obj(json!({
        "_tag": "McpAppRequestError",
        "threadId": thread_id,
        "reason": "session-stopped",
    }))
}

fn mcp_tool_info(payload: &Value) -> RpcExit {
    let tool = payload
        .get("tool")
        .or_else(|| payload.get("name"))
        .and_then(Value::as_str)
        .map_or("", str::trim);
    let mut out = serde_json::Map::from_iter([
        ("callable".to_owned(), Value::Bool(false)),
        ("readOnly".to_owned(), Value::Bool(true)),
    ]);
    if !tool.is_empty() {
        out.insert("title".to_owned(), Value::String(tool.to_owned()));
    }
    RpcExit::Success {
        value: Value::Object(out),
    }
}

fn mcp_read_resource(payload: &Value) -> RpcExit {
    let thread_id = payload
        .get("threadId")
        .and_then(Value::as_str)
        .unwrap_or("thread-default");
    rpc_fail_obj(json!({
        "_tag": "McpAppRequestError",
        "threadId": thread_id,
        "reason": "session-stopped",
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateModelContextPayload {
    thread_id: String,
    item_id: String,
    #[serde(default)]
    server: String,
    #[serde(default)]
    tool: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    content: Option<Value>,
}

fn mcp_update_model_context(state: &AppState, payload: Value) -> RpcExit {
    let req: UpdateModelContextPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let text = if req.text.is_empty() {
        req.content
            .as_ref()
            .and_then(|v| serde_json::to_string(v).ok())
            .unwrap_or_default()
    } else {
        req.text
    };
    let now = utc_now_iso();
    with_db(state, |conn| {
        let _ = conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS mcp_app_model_context (
               thread_id TEXT NOT NULL,
               item_id TEXT NOT NULL,
               server TEXT NOT NULL,
               tool TEXT NOT NULL,
               text TEXT NOT NULL,
               updated_at TEXT NOT NULL,
               PRIMARY KEY (thread_id, item_id)
             );",
        );
        conn.execute(
            "INSERT INTO mcp_app_model_context (thread_id, item_id, server, tool, text, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(thread_id, item_id) DO UPDATE SET
               server = excluded.server,
               tool = excluded.tool,
               text = excluded.text,
               updated_at = excluded.updated_at",
            params![req.thread_id, req.item_id, req.server, req.tool, text, now],
        )
        .map_err(|e| {
            rpc_fail_obj(json!({
                "_tag": "McpAppRequestError",
                "threadId": req.thread_id,
                "reason": e.to_string(),
            }))
        })?;
        Ok(RpcExit::Success { value: Value::Null })
    })
    .unwrap_or_else(|e| e)
}
