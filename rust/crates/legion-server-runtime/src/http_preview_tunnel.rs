//! `/api/preview/tunnel` handler enforcing preview tunnel policy and local port refusal headers (`#39415`).

use std::io::ErrorKind;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::Json;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{FromRequestParts, Query, Request, State};
use axum::http::header::HeaderName;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use legion_contracts::brand::ACTIVE_BRAND;
use legion_contracts::permissions::EnvironmentScope;
use legion_contracts::policy::allows_preview_tunnel;
use legion_protocol::tunnel::{
    PREVIEW_TUNNEL_DISABLED_SUFFIX, PREVIEW_TUNNEL_PORT_REFUSED_SUFFIX, PreviewTunnelStatusHeader,
    all_preview_tunnel_header_aliases,
};
use serde::Deserialize;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::error::ServerRuntimeError;
use crate::http_api::RuntimeHttpState;
use crate::http_auth_guard::authorize_request;

const TUNNEL_IO_BUF_BYTES: usize = 16 * 1024;

/// Query parameters accepted by `GET /api/preview/tunnel`.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PreviewTunnelQuery {
    pub port: Option<u16>,
}

/// Inserts canonical and brand-alias preview tunnel status headers with value `"1"`.
pub fn insert_tunnel_status_headers(headers: &mut HeaderMap, status: PreviewTunnelStatusHeader) {
    let suffix = match status {
        PreviewTunnelStatusHeader::Disabled => PREVIEW_TUNNEL_DISABLED_SUFFIX,
        PreviewTunnelStatusHeader::PortRefused => PREVIEW_TUNNEL_PORT_REFUSED_SUFFIX,
    };
    let mut names = vec![status.canonical_header().to_owned()];
    for candidate in ACTIVE_BRAND.tunnel_header_candidates(suffix) {
        if !names.contains(&candidate) {
            names.push(candidate);
        }
    }
    for alias in all_preview_tunnel_header_aliases(status) {
        if !names.contains(&alias) {
            names.push(alias);
        }
    }
    for name_str in names {
        if let Ok(name) = HeaderName::from_bytes(name_str.as_bytes()) {
            headers.insert(name, HeaderValue::from_static("1"));
        }
    }
}

/// Builds the disabled response (`403` + `x-legioncode-tunnel-disabled: 1` and brand alias headers).
pub fn tunnel_disabled_response() -> Response {
    let body = format!(
        "The preview tunnel is turned off on this {} server.",
        ACTIVE_BRAND.product_name
    );
    let mut resp = (StatusCode::FORBIDDEN, body).into_response();
    insert_tunnel_status_headers(resp.headers_mut(), PreviewTunnelStatusHeader::Disabled);
    resp
}

/// Builds the local port-refused response (`502` + `x-legioncode-tunnel-port-refused: 1` and brand alias headers).
pub fn tunnel_port_refused_response(port: u16) -> Response {
    let body = format!("Local preview target 127.0.0.1:{port} refused connection.");
    let mut resp = (StatusCode::BAD_GATEWAY, body).into_response();
    insert_tunnel_status_headers(resp.headers_mut(), PreviewTunnelStatusHeader::PortRefused);
    resp
}

/// `GET/POST /api/preview/tunnel` — enforces `allows_preview_tunnel`, dials loopback `port`, and
/// upgrades WebSocket streams or reports local `ECONNREFUSED` with structured headers.
pub(crate) async fn handle_preview_tunnel(
    State(state): State<Arc<RuntimeHttpState>>,
    req: Request,
) -> Result<Response, ServerRuntimeError> {
    let (mut parts, _body) = req.into_parts();
    let _principal = authorize_request(
        &state,
        &parts.headers,
        &[EnvironmentScope::TerminalOperate],
        false,
    )?;

    if !allows_preview_tunnel(state.enable_preview_tunnel, state.managed_policy.as_ref()) {
        return Ok(tunnel_disabled_response());
    }

    let Query(query) = Query::<PreviewTunnelQuery>::from_request_parts(&mut parts, &state)
        .await
        .map_err(|err| ServerRuntimeError::BadRequest(err.to_string()))?;

    let Some(port) = query.port.filter(|&p| p > 0) else {
        return Err(ServerRuntimeError::BadRequest(
            "Valid loopback `port` (1..=65535) query parameter is required".to_owned(),
        ));
    };

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let stream = match TcpStream::connect(addr).await {
        Ok(stream) => stream,
        Err(err) if err.kind() == ErrorKind::ConnectionRefused => {
            return Ok(tunnel_port_refused_response(port));
        }
        Err(err) => {
            return Ok((
                StatusCode::BAD_GATEWAY,
                format!("Failed to connect preview tunnel to 127.0.0.1:{port}: {err}"),
            )
                .into_response());
        }
    };

    if let Ok(upgrade) = WebSocketUpgrade::from_request_parts(&mut parts, &state).await {
        return Ok(upgrade
            .on_upgrade(move |socket| async move {
                bridge_ws_and_tcp(socket, stream).await;
            })
            .into_response());
    }

    drop(stream);
    Ok((
        StatusCode::OK,
        Json(json!({
            "ok": true,
            "port": port,
            "allowed": true,
        })),
    )
        .into_response())
}

async fn bridge_ws_and_tcp(socket: WebSocket, stream: TcpStream) {
    let (mut ws_tx, mut ws_rx) = socket.split();
    let (mut tcp_rx, mut tcp_tx) = stream.into_split();

    let ws_to_tcp = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_rx.next().await {
            match msg {
                Message::Binary(bytes) => {
                    if tcp_tx.write_all(&bytes).await.is_err() {
                        break;
                    }
                }
                Message::Text(text) => {
                    if tcp_tx.write_all(text.as_bytes()).await.is_err() {
                        break;
                    }
                }
                Message::Close(_) => break,
                Message::Ping(_) | Message::Pong(_) => {}
            }
        }
        let _ = tcp_tx.shutdown().await;
    });

    let tcp_to_ws = tokio::spawn(async move {
        let mut buf = vec![0u8; TUNNEL_IO_BUF_BYTES];
        loop {
            match tcp_rx.read(&mut buf).await {
                Ok(0) | Err(_) => {
                    let _ = ws_tx.close().await;
                    break;
                }
                Ok(n) => {
                    let payload = buf[..n].to_vec();
                    if ws_tx.send(Message::Binary(payload.into())).await.is_err() {
                        break;
                    }
                }
            }
        }
    });

    let _ = tokio::join!(ws_to_tcp, tcp_to_ws);
}
