//! `acp-mcp-call` tool invocation and `acp-mcp-bridge` stdio-to-HTTP relay.

use std::sync::Arc;

use serde_json::{Value, json};
use t3_contracts::brand::ACTIVE_BRAND;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::Mutex;
use tokio::task::JoinSet;

use crate::acp_mcp_bridge::{McpBridgeError, McpHttpClient, strip_duplicate_structured_text};

fn acp_cli_client_name() -> String {
    let product_slug = ACTIVE_BRAND
        .product_name
        .to_ascii_lowercase()
        .replace(' ', "-");
    format!("{product_slug}-acp-cli")
}

/// Calls a single MCP tool (`acp-mcp-call`), normalizing duplicated `structuredContent` text and closing via `DELETE`.
pub async fn call_mcp_tool(
    endpoint: &str,
    authorization: &str,
    tool: &str,
    args_json: &str,
) -> Result<Value, McpBridgeError> {
    let parsed: Value =
        serde_json::from_str(args_json).map_err(|_| McpBridgeError::InvalidInput {
            detail: "acp-mcp-call arguments must be valid JSON.".to_owned(),
        })?;
    if !parsed.is_object() {
        return Err(McpBridgeError::InvalidInput {
            detail: "acp-mcp-call arguments must be a JSON object.".to_owned(),
        });
    }
    let client_name = acp_cli_client_name();
    let client = McpHttpClient::connect(endpoint, authorization, &client_name).await?;
    let outcome = client.call_tool(tool, &parsed).await;
    client.close().await;
    let mut result = outcome?;
    strip_duplicate_structured_text(&mut result);
    Ok(result)
}

/// Bridges NDJSON lines from `reader` onto HTTP/SSE `/mcp` and writes responses to `writer` (`acp-mcp-bridge`).
///
/// Orders only session establishment (`initialize` and `notifications/initialized`) while executing
/// subsequent tool requests and JSON-RPC responses concurrently (`AcpMcpStdioBridge.ts:272-342`).
pub async fn run_mcp_stdio_bridge<R, W>(
    endpoint: &str,
    authorization: &str,
    reader: R,
    writer: W,
) -> Result<(), McpBridgeError>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let client = Arc::new(McpHttpClient::new_uninitialized(endpoint, authorization)?);
    let output = Arc::new(Mutex::new(writer));
    let handshake_lock = Arc::new(Mutex::new(()));
    let mut tasks = JoinSet::new();
    let mut lines = reader.lines();

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(envelope) = serde_json::from_str::<Value>(&line) else {
            write_ndjson(
                &output,
                &json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": "Parse error" } }),
            )
            .await?;
            continue;
        };
        if !envelope.is_object() {
            continue;
        }
        let is_response = envelope.get("id").is_some()
            && (envelope.get("result").is_some() || envelope.get("error").is_some());
        let is_handshake = matches!(
            envelope.get("method").and_then(Value::as_str),
            Some("initialize" | "notifications/initialized")
        );

        let client_ref = Arc::clone(&client);
        let output_ref = Arc::clone(&output);
        if is_handshake {
            let guard = Arc::clone(&handshake_lock).lock_owned().await;
            tasks.spawn(async move {
                let _hold = guard;
                forward_line_task(&client_ref, &output_ref, &envelope).await;
            });
        } else if is_response {
            tasks.spawn(async move {
                forward_line_task(&client_ref, &output_ref, &envelope).await;
            });
        } else {
            let gate = Arc::clone(&handshake_lock);
            tasks.spawn(async move {
                drop(gate.lock().await);
                forward_line_task(&client_ref, &output_ref, &envelope).await;
            });
        }
    }

    while tasks.join_next().await.is_some() {}
    client.terminate_session().await;
    Ok(())
}

async fn forward_line_task<W: AsyncWrite + Unpin>(
    client: &McpHttpClient,
    output: &Arc<Mutex<W>>,
    envelope: &Value,
) {
    match client.forward_raw(envelope).await {
        Ok(messages) => {
            for msg in messages {
                let _ = write_ndjson(output, &msg).await;
            }
        }
        Err(err) => {
            if let Some(id) = envelope.get("id") {
                let err_msg = match err {
                    McpBridgeError::HttpStatus { .. } => err.to_string(),
                    other => format!(
                        "{} MCP bridge request failed: {other}",
                        ACTIVE_BRAND.product_name
                    ),
                };
                let _ = write_ndjson(
                    output,
                    &json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32603, "message": err_msg } }),
                )
                .await;
            }
        }
    }
}

async fn write_ndjson<W: AsyncWrite + Unpin>(
    output: &Arc<Mutex<W>>,
    message: &Value,
) -> Result<(), std::io::Error> {
    let encoded = serde_json::to_string(message).unwrap_or_else(|_| "null".to_owned());
    let mut guard = output.lock().await;
    guard.write_all(encoded.as_bytes()).await?;
    guard.write_all(b"\n").await?;
    guard.flush().await
}
