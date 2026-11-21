#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use t3_remote::live_network::{
    LiveCloudRelayTunnel, RelayChunkEncoding, RelayHttpRequestEnvelope, RelayWsOutgoingFrame,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

async fn read_req(stream: &mut TcpStream) -> Option<(String, String, Vec<u8>)> {
    let mut raw = Vec::new();
    let mut buf = [0_u8; 4096];
    let head_end;
    loop {
        let n = stream.read(&mut buf).await.ok()?;
        if n == 0 {
            return None;
        }
        raw.extend_from_slice(&buf[..n]);
        if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            head_end = pos + 4;
            break;
        }
    }
    let head = String::from_utf8_lossy(&raw[..head_end - 4]);
    let mut lines = head.lines();
    let first = lines.next()?;
    let mut parts = first.split_whitespace();
    let method = parts.next()?.to_owned();
    let path = parts.next()?.to_owned();
    let mut content_len = 0_usize;
    for line in lines {
        if let Some((k, v)) = line.split_once(':')
            && k.trim().eq_ignore_ascii_case("content-length")
        {
            content_len = v.trim().parse().unwrap_or(0);
        }
    }
    while raw.len() - head_end < content_len {
        let n = stream.read(&mut buf).await.ok()?;
        if n == 0 {
            break;
        }
        raw.extend_from_slice(&buf[..n]);
    }
    Some((method, path, raw[head_end..head_end + content_len].to_vec()))
}

async fn spawn_local_origin_server()
-> Result<(u16, watch::Sender<bool>), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let (tx, mut rx) = watch::channel(false);

    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = rx.changed() => if *rx.borrow() { break; },
                accepted = listener.accept() => {
                    let Ok((mut stream, _)) = accepted else { break; };
                    tokio::spawn(async move {
                        let Some((method, path, body)) = read_req(&mut stream).await else { return; };
                        match (method.as_str(), path.as_str()) {
                            ("GET", "/hello") => {
                                let json = r#"{"ok":true,"origin":"local"}"#;
                                let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json}", json.len());
                                let _ = stream.write_all(resp.as_bytes()).await;
                            }
                            ("POST", "/echo") => {
                                let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                                let _ = stream.write_all(resp.as_bytes()).await;
                                let _ = stream.write_all(&body).await;
                            }
                            ("GET", "/stream") => {
                                let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
                                let _ = stream.write_all(head.as_bytes()).await;
                                let _ = stream.write_all(b"d\r\ndata: part-1\n\r\n").await;
                                let _ = stream.flush().await;
                                tokio::task::yield_now().await;
                                let _ = stream.write_all(b"e\r\ndata: part-2\n\n\r\n0\r\n\r\n").await;
                            }
                            _ => {
                                let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                            }
                        }
                    });
                }
            }
        }
    });
    Ok((port, tx))
}

#[tokio::test]
async fn live_cloud_relay_tunnel_forwards_http_and_streams_chunks_over_ws()
-> Result<(), Box<dyn std::error::Error>> {
    let (local_port, stop_origin) = spawn_local_origin_server().await?;
    let ws_listener = TcpListener::bind("127.0.0.1:0").await?;
    let ws_port = ws_listener.local_addr()?.port();

    let relay_task = tokio::spawn(async move {
        let (stream, _) = ws_listener.accept().await.map_err(|e| e.to_string())?;
        let mut peek_buf = [0_u8; 2048];
        let mut peek_len = 0_usize;
        for _ in 0..32 {
            let n = stream
                .peek(&mut peek_buf)
                .await
                .map_err(|e| e.to_string())?;
            peek_len = n;
            if peek_buf[..n].windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
            tokio::task::yield_now().await;
        }
        let handshake = String::from_utf8_lossy(&peek_buf[..peek_len]).to_ascii_lowercase();
        assert!(handshake.contains("authorization: bearer relay-secret-tok"));

        let mut ws = accept_async(stream).await.map_err(|e| e.to_string())?;

        let requests = [
            ("r-hello", "GET", "/hello", None),
            ("r-echo", "POST", "/echo", Some(json!({ "msg": "pong" }))),
            ("r-stream", "GET", "/stream", None),
            ("r-bad", "GET", "//evil.example/steal", None),
        ];
        for (id, method, path, payload) in &requests {
            let env = RelayHttpRequestEnvelope {
                request_id: (*id).to_owned(),
                method: (*method).to_owned(),
                path: (*path).to_owned(),
                headers: BTreeMap::new(),
                body: None,
                json: payload.clone(),
            };
            let txt = serde_json::to_string(&env).map_err(|e| e.to_string())?;
            ws.send(Message::Text(txt.into()))
                .await
                .map_err(|e| e.to_string())?;
        }

        let mut ended = 0_usize;
        let mut frames = Vec::new();
        while ended < requests.len() {
            if let Some(Ok(Message::Text(txt))) = ws.next().await {
                let frame: RelayWsOutgoingFrame =
                    serde_json::from_str(txt.as_str()).map_err(|e| e.to_string())?;
                if matches!(
                    frame,
                    RelayWsOutgoingFrame::ResponseEnd { .. }
                        | RelayWsOutgoingFrame::ResponseError { .. }
                ) {
                    ended += 1;
                }
                frames.push(frame);
            }
        }
        let _ = ws.send(Message::Close(None)).await;
        Ok::<Vec<RelayWsOutgoingFrame>, String>(frames)
    });

    let tunnel = LiveCloudRelayTunnel::connect_with_bearer(
        format!("ws://127.0.0.1:{ws_port}"),
        local_port,
        Some("relay-secret-tok"),
    )
    .await?;
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);
    tunnel.run_until_shutdown(shutdown_rx).await?;

    let frames = relay_task.await??;
    assert!(frames.iter().any(|f| matches!(
        f,
        RelayWsOutgoingFrame::ResponseChunk { request_id, data, encoding }
            if request_id == "r-hello"
                && *encoding == RelayChunkEncoding::Utf8
                && data.contains("local")
    )));
    assert!(frames.iter().any(|f| matches!(
        f,
        RelayWsOutgoingFrame::ResponseChunk { request_id, data, .. }
            if request_id == "r-echo" && data.contains("pong")
    )));
    let stream_text: String = frames
        .iter()
        .filter_map(|f| match f {
            RelayWsOutgoingFrame::ResponseChunk {
                request_id, data, ..
            } if request_id == "r-stream" => Some(data.as_str()),
            _ => None,
        })
        .collect();
    assert!(stream_text.contains("data: part-1"));
    assert!(stream_text.contains("data: part-2"));
    assert!(frames.iter().any(|f| matches!(
        f,
        RelayWsOutgoingFrame::ResponseError { request_id, status, .. }
            if request_id == "r-bad" && *status == 400
    )));

    let _ = stop_origin.send(true);
    Ok(())
}
