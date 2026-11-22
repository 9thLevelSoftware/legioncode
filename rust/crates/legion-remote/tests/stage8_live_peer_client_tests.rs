#![forbid(unsafe_code)]

use legion_contracts::environment::{
    ExecutionEnvironmentDescriptor, PlatformArch, PlatformInfo, PlatformOs,
};
use legion_contracts::ids::EnvironmentId;
use legion_contracts::orchestration::McpClientAccess;
use legion_persistence::sqlite::SqliteStore;
use legion_remote::live_network::{LivePeerClient, LivePeerLinkInput};
use legion_remote::peer::links::{PeerLinkErrorReason, PeerLinkStatus, PeerLinksService};
use legion_remote::secrets::FileSecretStore;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

async fn read_http(stream: &mut TcpStream) -> Option<(String, String, Vec<u8>)> {
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
    let head = String::from_utf8_lossy(&raw[..header_end_idx(head_end)]);
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

const fn header_end_idx(head_end: usize) -> usize {
    head_end.saturating_sub(4)
}

async fn spawn_mock_pair_server() -> Result<(u16, watch::Sender<bool>), Box<dyn std::error::Error>>
{
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
                        let Some((method, path, body)) = read_http(&mut stream).await else { return; };
                        let (status, json) = if method == "GET" && path == "/.well-known/legioncode/environment" {
                            let desc = ExecutionEnvironmentDescriptor::builder(
                                EnvironmentId::new("env-remote-peer").unwrap_or_else(|_| unreachable!()),
                                "Remote Peer",
                                PlatformInfo::new(PlatformOs::Linux, PlatformArch::X64),
                                "0.1.10",
                            )
                            .build();
                            ("200 OK", serde_json::to_string(&desc).unwrap_or_default())
                        } else if method == "POST" && path == "/api/auth/pair" {
                            let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
                            match parsed.get("token").and_then(serde_json::Value::as_str) {
                                Some("pair-secret-123") => (
                                    "200 OK",
                                    serde_json::json!({
                                        "accessToken": "bearer-peer-tok",
                                        "tokenType": "Bearer",
                                        "sessionId": "sess-peer-1",
                                        "scopes": ["mcp"],
                                        "role": "member",
                                        "expiresAt": "2099-01-01T00:00:00.000Z",
                                        "expiresIn": 3600
                                    })
                                    .to_string(),
                                ),
                                Some("pair-dual-keys") => (
                                    "200 OK",
                                    serde_json::json!({
                                        "accessToken": "dual-key-tok",
                                        "access_token": "dual-key-tok",
                                        "tokenType": "Bearer",
                                        "token_type": "Bearer",
                                        "sessionId": "sess-dual-1",
                                        "scopes": ["mcp"],
                                        "role": "member",
                                        "expiresAt": "2099-06-01T00:00:00.000Z",
                                        "expires_at": "2099-06-01T00:00:00.000Z",
                                        "expiresIn": 3600,
                                        "expires_in": 3600
                                    })
                                    .to_string(),
                                ),
                                _ => ("401 Unauthorized", "bad token".to_owned()),
                            }
                        } else {
                            ("404 Not Found", "{}".to_owned())
                        };
                        let resp = format!(
                            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json}",
                            json.len()
                        );
                        let _ = stream.write_all(resp.as_bytes()).await;
                    });
                }
            }
        }
    });
    Ok((port, tx))
}

#[tokio::test]
async fn live_peer_client_links_and_lists_live_summaries() -> Result<(), Box<dyn std::error::Error>>
{
    let (port, tx) = spawn_mock_pair_server().await?;

    let dir = tempfile::tempdir()?;
    let store = SqliteStore::open_in_memory()?;
    let secrets = FileSecretStore::from_state_dir(dir.path())?;
    let client = LivePeerClient::new()?;

    let mut bad = LivePeerLinkInput::new(
        "http://public.example.com",
        "pair-secret-123",
        McpClientAccess::Auto,
        "LocalBox",
    );
    bad.self_environment_id = Some(EnvironmentId::new("env-self")?);
    let err = client
        .link_peer_over_http(store.connection(), &secrets, bad)
        .await
        .expect_err("public HTTP must be refused");
    assert_eq!(err.reason, PeerLinkErrorReason::Unreachable);

    let origin = format!("http://127.0.0.1:{port}");
    let mut input = LivePeerLinkInput::new(
        &origin,
        "pair-secret-123",
        McpClientAccess::Auto,
        "LocalBox",
    );
    input.linked_at = Some("2026-11-20T00:00:00.000Z".to_owned());
    let linked = client
        .link_peer_over_http(store.connection(), &secrets, input)
        .await?;
    assert_eq!(linked.environment_id.as_str(), "env-remote-peer");

    let live = client
        .list_live_peer_summaries(store.connection(), &secrets, "2026-11-20T01:00:00.000Z")
        .await?;
    assert_eq!(live[0].status, PeerLinkStatus::Reachable);

    let expired = client
        .list_live_peer_summaries(store.connection(), &secrets, "2099-02-01T00:00:00.000Z")
        .await?;
    assert_eq!(expired[0].status, PeerLinkStatus::Expired);

    let mut dual_input =
        LivePeerLinkInput::new(&origin, "pair-dual-keys", McpClientAccess::Auto, "LocalBox");
    dual_input.linked_at = Some("2026-11-20T00:00:00.000Z".to_owned());
    let dual_linked = client
        .link_peer_over_http(store.connection(), &secrets, dual_input)
        .await?;
    assert_eq!(dual_linked.expires_at, "2099-06-01T00:00:00.000Z");
    let resolved = PeerLinksService::new(secrets.clone()).resolve_credentials(
        store.connection(),
        "env-remote-peer",
        "2026-11-20T01:00:00.000Z",
    )?;
    assert_eq!(resolved.token, "dual-key-tok");

    let _ = tx.send(true);
    Ok(())
}
