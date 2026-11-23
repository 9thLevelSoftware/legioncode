//! Local HTTP envelope forwarding and multiplexed WebSocket relay loop.

use std::collections::BTreeMap;

use futures_util::{SinkExt, StreamExt};
use reqwest::header::{HeaderName, HeaderValue};
use reqwest::{Client, Method};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use super::types::{
    LiveCloudRelayError, RelayChunkEncoding, RelayHttpRequestEnvelope, RelayWsOutgoingFrame,
};

/// Validates an envelope path so relay traffic cannot escape `http://127.0.0.1:<port>`.
pub(super) fn validate_local_forward_path(path: &str) -> Result<&str, String> {
    if !path.starts_with('/')
        || path.starts_with("//")
        || path.contains("://")
        || path.contains('\\')
    {
        return Err(format!("invalid local relay path `{path}`"));
    }
    Ok(path)
}

fn is_hop_by_hop_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "host" | "connection" | "content-length" | "transfer-encoding" | "upgrade" | "keep-alive"
    )
}

/// Streams a single incoming [`RelayHttpRequestEnvelope`] against `http://127.0.0.1:<local_port>`.
pub(super) async fn forward_local_http_envelope(
    http: Client,
    local_port: u16,
    envelope: RelayHttpRequestEnvelope,
    tx: mpsc::Sender<RelayWsOutgoingFrame>,
) {
    let request_id = envelope.request_id.clone();
    if let Err((status, message)) = run_forward(&http, local_port, envelope, &tx).await {
        let _ = tx
            .send(RelayWsOutgoingFrame::ResponseError {
                request_id,
                status,
                message,
            })
            .await;
    }
}

async fn run_forward(
    http: &Client,
    local_port: u16,
    envelope: RelayHttpRequestEnvelope,
    tx: &mpsc::Sender<RelayWsOutgoingFrame>,
) -> Result<(), (u16, String)> {
    let path = validate_local_forward_path(&envelope.path).map_err(|msg| (400, msg))?;
    let method = Method::from_bytes(envelope.method.as_bytes())
        .map_err(|err| (400, format!("invalid HTTP method: {err}")))?;
    let target_url = format!("http://127.0.0.1:{local_port}{path}");

    let mut builder = http.request(method, &target_url);
    for (k, v) in &envelope.headers {
        if is_hop_by_hop_header(k) {
            continue;
        }
        if let (Ok(name), Ok(val)) = (
            HeaderName::from_bytes(k.as_bytes()),
            HeaderValue::from_str(v),
        ) {
            builder = builder.header(name, val);
        }
    }

    if let Some(raw_body) = envelope.body {
        builder = builder.body(raw_body);
    } else if let Some(json_val) = envelope.json {
        builder = builder.json(&json_val);
    }

    let mut resp = builder
        .send()
        .await
        .map_err(|err| (502, format!("local origin unreachable: {err}")))?;

    let mut resp_headers = BTreeMap::new();
    for (k, v) in resp.headers() {
        if let Ok(val) = v.to_str() {
            resp_headers.insert(k.as_str().to_owned(), val.to_owned());
        }
    }
    let _ = tx
        .send(RelayWsOutgoingFrame::ResponseHead {
            request_id: envelope.request_id.clone(),
            status: resp.status().as_u16(),
            headers: resp_headers,
        })
        .await;

    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|err| (502, format!("failed reading local origin stream: {err}")))?
    {
        let (data, encoding) = match std::str::from_utf8(&chunk) {
            Ok(utf8) => (utf8.to_owned(), RelayChunkEncoding::Utf8),
            Err(_) => (hex::encode(&chunk), RelayChunkEncoding::Hex),
        };
        let _ = tx
            .send(RelayWsOutgoingFrame::ResponseChunk {
                request_id: envelope.request_id.clone(),
                data,
                encoding,
            })
            .await;
    }

    let _ = tx
        .send(RelayWsOutgoingFrame::ResponseEnd {
            request_id: envelope.request_id,
        })
        .await;
    Ok(())
}

/// Runs the multiplexed WebSocket relay loop forwarding requests to `http://127.0.0.1:<local_port>`.
pub(super) async fn run_relay_ws_loop(
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    http: Client,
    local_port: u16,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), LiveCloudRelayError> {
    let (mut ws_sink, mut ws_stream) = ws.split();
    let (tx, mut rx) = mpsc::channel::<RelayWsOutgoingFrame>(64);

    loop {
        if *shutdown.borrow() {
            let _ = ws_sink.send(Message::Close(None)).await;
            break;
        }
        tokio::select! {
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    let _ = ws_sink.send(Message::Close(None)).await;
                    break;
                }
            }
            maybe_frame = rx.recv() => {
                if let Some(frame) = maybe_frame {
                    let encoded = serde_json::to_string(&frame)?;
                    ws_sink
                        .send(Message::Text(encoded.into()))
                        .await
                        .map_err(|err| LiveCloudRelayError::WebSocket { detail: err.to_string() })?;
                }
            }
            maybe_msg = ws_stream.next() => {
                match maybe_msg {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<RelayHttpRequestEnvelope>(text.as_str()) {
                            Ok(env) => {
                                let fwd_http = http.clone();
                                let fwd_tx = tx.clone();
                                tokio::spawn(async move {
                                    forward_local_http_envelope(fwd_http, local_port, env, fwd_tx).await;
                                });
                            }
                            Err(err) => {
                                let _ = tx.send(RelayWsOutgoingFrame::ResponseError {
                                    request_id: String::new(),
                                    status: 400,
                                    message: format!("invalid relay request envelope: {err}"),
                                }).await;
                            }
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        ws_sink
                            .send(Message::Pong(payload))
                            .await
                            .map_err(|err| LiveCloudRelayError::WebSocket { detail: err.to_string() })?;
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {}
                    Some(Err(err)) => {
                        return Err(LiveCloudRelayError::WebSocket { detail: err.to_string() });
                    }
                }
            }
        }
    }
    Ok(())
}
