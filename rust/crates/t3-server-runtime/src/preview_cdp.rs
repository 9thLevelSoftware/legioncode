//! Chrome `DevTools` Protocol (CDP) WebSocket JSON-RPC client over `tokio-tungstenite`.

use std::sync::atomic::{AtomicU64, Ordering};

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

use crate::error::ServerRuntimeError;

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Mouse input parameters for `Input.dispatchMouseEvent`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CdpMouseEventParams {
    /// Event type (`"mousePressed"`, `"mouseReleased"`, `"mouseMoved"`).
    #[serde(rename = "type", alias = "eventType")]
    pub event_type: String,
    /// X coordinate in CSS pixels.
    pub x: f64,
    /// Y coordinate in CSS pixels.
    pub y: f64,
    /// Mouse button (`"left"`, `"right"`, `"middle"`, `"none"`).
    #[serde(default = "default_mouse_button")]
    pub button: String,
    /// Click count (`1` for single click).
    #[serde(default = "default_click_count")]
    pub click_count: u32,
}

fn default_mouse_button() -> String {
    "left".to_owned()
}

const fn default_click_count() -> u32 {
    1
}

/// Key input parameters for `Input.dispatchKeyEvent`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CdpKeyEventParams {
    /// Event type (`"keyDown"`, `"keyUp"`, `"rawKeyDown"`, `"char"`).
    #[serde(rename = "type", alias = "eventType")]
    pub event_type: String,
    /// Key text generated (optional).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// DOM `KeyboardEvent.key` string (optional).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

/// Active CDP WebSocket JSON-RPC connection supporting optional flattened target `sessionId`.
pub struct CdpConnection {
    ws: WsStream,
    next_id: AtomicU64,
    session_id: Option<String>,
}

impl CdpConnection {
    /// Connect to a `ws://` or `wss://` Chrome `DevTools` Protocol WebSocket endpoint.
    pub async fn connect(ws_url: &str) -> Result<Self, ServerRuntimeError> {
        let (ws, _resp) = connect_async(ws_url).await.map_err(|e| {
            ServerRuntimeError::Process(format!("CDP connect `{ws_url}` failed: {e}"))
        })?;
        Ok(Self {
            ws,
            next_id: AtomicU64::new(1),
            session_id: None,
        })
    }

    /// Attach a flattened target `sessionId` for subsequent page-scoped CDP commands.
    pub fn set_session_id(&mut self, session_id: Option<String>) {
        self.session_id = session_id;
    }

    /// Send one CDP JSON-RPC command and await the matching response `id`.
    pub async fn call(&mut self, method: &str, params: Value) -> Result<Value, ServerRuntimeError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut frame = json!({
            "id": id,
            "method": method,
            "params": params,
        });
        if let Some(ref sid) = self.session_id
            && !method.starts_with("Target.")
            && let Some(obj) = frame.as_object_mut()
        {
            obj.insert("sessionId".to_owned(), Value::String(sid.clone()));
        }
        let encoded = serde_json::to_string(&frame)?;
        self.ws
            .send(Message::text(encoded))
            .await
            .map_err(|e| ServerRuntimeError::Process(format!("CDP send failed: {e}")))?;

        while let Some(msg_res) = self.ws.next().await {
            let msg = msg_res
                .map_err(|e| ServerRuntimeError::Process(format!("CDP recv failed: {e}")))?;
            if msg.is_close() {
                return Err(ServerRuntimeError::Process(
                    "CDP WebSocket closed before replying".to_owned(),
                ));
            }
            let text = match msg {
                Message::Text(t) => t.to_string(),
                Message::Binary(b) => String::from_utf8(b.to_vec()).map_err(|_| {
                    ServerRuntimeError::Process("invalid UTF-8 CDP frame".to_owned())
                })?,
                _ => continue,
            };
            let parsed: Value = serde_json::from_str(&text)?;
            let matches_id = parsed
                .get("id")
                .and_then(Value::as_u64)
                .is_some_and(|resp_id| resp_id == id);
            if !matches_id {
                continue;
            }
            if let Some(err) = parsed.get("error") {
                return Err(ServerRuntimeError::Process(format!("CDP error: {err}")));
            }
            return Ok(parsed.get("result").cloned().unwrap_or(Value::Null));
        }
        Err(ServerRuntimeError::Process(
            "CDP stream ended before response".to_owned(),
        ))
    }

    /// Run `Target.createTarget` for `url` and return the created `targetId`.
    pub async fn create_target(&mut self, url: &str) -> Result<String, ServerRuntimeError> {
        let res = self
            .call("Target.createTarget", json!({ "url": url }))
            .await?;
        res.get("targetId")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .ok_or_else(|| {
                ServerRuntimeError::Process("Target.createTarget missing targetId".to_owned())
            })
    }

    /// Run `Page.enable`.
    pub async fn page_enable(&mut self) -> Result<Value, ServerRuntimeError> {
        self.call("Page.enable", json!({})).await
    }

    /// Run `Page.navigate`.
    pub async fn navigate(&mut self, url: &str) -> Result<Value, ServerRuntimeError> {
        self.call("Page.navigate", json!({ "url": url })).await
    }

    /// Run `Runtime.evaluate` with `returnByValue: true`.
    pub async fn evaluate(&mut self, expression: &str) -> Result<Value, ServerRuntimeError> {
        self.call(
            "Runtime.evaluate",
            json!({
                "expression": expression,
                "returnByValue": true,
                "awaitPromise": true,
            }),
        )
        .await
    }

    /// Run `Page.captureScreenshot` returning base64 image data.
    pub async fn capture_screenshot(
        &mut self,
        format: Option<&str>,
    ) -> Result<Value, ServerRuntimeError> {
        let fmt = format.unwrap_or("png");
        self.call("Page.captureScreenshot", json!({ "format": fmt }))
            .await
    }

    /// Run `Input.dispatchMouseEvent`.
    pub async fn dispatch_mouse_event(
        &mut self,
        params: &CdpMouseEventParams,
    ) -> Result<Value, ServerRuntimeError> {
        self.call("Input.dispatchMouseEvent", serde_json::to_value(params)?)
            .await
    }

    /// Run `Input.dispatchKeyEvent`.
    pub async fn dispatch_key_event(
        &mut self,
        params: &CdpKeyEventParams,
    ) -> Result<Value, ServerRuntimeError> {
        self.call("Input.dispatchKeyEvent", serde_json::to_value(params)?)
            .await
    }

    /// Run `Emulation.setDeviceMetricsOverride`.
    pub async fn set_device_metrics(
        &mut self,
        width: u32,
        height: u32,
        device_scale_factor: f64,
        mobile: bool,
    ) -> Result<Value, ServerRuntimeError> {
        self.call(
            "Emulation.setDeviceMetricsOverride",
            json!({
                "width": width,
                "height": height,
                "deviceScaleFactor": device_scale_factor,
                "mobile": mobile,
            }),
        )
        .await
    }

    /// Close the underlying CDP WebSocket cleanly.
    pub async fn close(&mut self) {
        let _ = self.ws.close(None).await;
    }
}
