#![forbid(unsafe_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use legion_remote::live_network::{
    CLOUD_LINK_REVOKED, CLOUD_LINKED_USER_ID, CloudCliLinkMode, CloudConnectError,
    CloudConnectExposure, CloudConnectManager, CloudDisconnectOptions, CloudLinkProvisionState,
    DeviceAuthorizationConfig, PUBLISH_AGENT_ACTIVITY_SECRET, RELAY_URL_SECRET, RelayUnlinkOutcome,
};
use legion_remote::secrets::FileSecretStore;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

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

async fn spawn_mock_oauth_server() -> Result<
    (u16, Arc<AtomicUsize>, Arc<AtomicUsize>, watch::Sender<bool>),
    Box<dyn std::error::Error>,
> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let polls = Arc::new(AtomicUsize::new(0));
    let revoked_calls = Arc::new(AtomicUsize::new(0));
    let (stop_tx, mut stop_rx) = watch::channel(false);

    let polls_srv = Arc::clone(&polls);
    let revoked_srv = Arc::clone(&revoked_calls);
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = stop_rx.changed() => if *stop_rx.borrow() { break; },
                accepted = listener.accept() => {
                    let Ok((mut stream, _)) = accepted else { break; };
                    let polls_c = Arc::clone(&polls_srv);
                    let rev_c = Arc::clone(&revoked_srv);
                    tokio::spawn(async move {
                        let Some((method, path, body)) = read_req(&mut stream).await else { return; };
                        let (status, json_body) = match (method.as_str(), path.as_str()) {
                            ("POST", "/oauth/device_authorization") => (
                                "200 OK",
                                json!({
                                    "device_code": "dev-code-1",
                                    "user_code": "ABCD-EFGH",
                                    "verification_uri": "https://legion.codes/device",
                                    "verification_uri_complete": "https://legion.codes/device?code=ABCD-EFGH",
                                    "expires_in": 300,
                                    "interval": 1
                                }),
                            ),
                            ("POST", "/oauth/token") => {
                                let raw = String::from_utf8_lossy(&body);
                                if raw.contains("denied-code") {
                                    ("400 Bad Request", json!({ "error": "access_denied" }))
                                } else {
                                    match polls_c.fetch_add(1, Ordering::Relaxed) {
                                        0 => ("400 Bad Request", json!({ "error": "authorization_pending" })),
                                        1 => ("400 Bad Request", json!({ "error": "slow_down" })),
                                        _ => ("200 OK", json!({
                                            "access_token": "cli-tok-99",
                                            "refresh_token": "ref-99",
                                            "expires_in": 3600,
                                            "id_token": "alice@example.com"
                                        })),
                                    }
                                }
                            }
                            ("DELETE", "/v1/client/environment-links/env-cloud-1") => {
                                rev_c.fetch_add(1, Ordering::Relaxed);
                                ("200 OK", json!({ "ok": true }))
                            }
                            _ => ("404 Not Found", Value::Null),
                        };
                        let payload = json_body.to_string();
                        let resp = format!(
                            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                            payload.len()
                        );
                        let _ = stream.write_all(resp.as_bytes()).await;
                    });
                }
            }
        }
    });

    Ok((port, polls, revoked_calls, stop_tx))
}

#[tokio::test]
async fn cloud_connect_manager_oauth_device_poll_secrets_and_disconnect()
-> Result<(), Box<dyn std::error::Error>> {
    let (port, polls, revoked_calls, stop_tx) = spawn_mock_oauth_server().await?;
    let dir = tempfile::tempdir()?;
    let secrets = FileSecretStore::from_state_dir(dir.path())?;
    let mgr = CloudConnectManager::new(secrets.clone())?;
    let origin = format!("http://127.0.0.1:{port}");
    let cfg = DeviceAuthorizationConfig {
        device_authorization_endpoint: format!("{origin}/oauth/device_authorization"),
        token_endpoint: format!("{origin}/oauth/token"),
        client_id: "cli_123".to_owned(),
        scopes: vec!["openid".to_owned(), "offline_access".to_owned()],
        request_timeout: Duration::from_secs(5),
        slow_down_increment: Duration::from_millis(5),
    };

    let prompt = mgr.start_device_authorization(&cfg).await?;
    assert_eq!(prompt.user_code, "ABCD-EFGH");

    let denied = mgr
        .poll_device_token(
            &cfg,
            "denied-code",
            Duration::from_millis(5),
            Duration::from_secs(2),
        )
        .await;
    assert!(matches!(
        denied,
        Err(CloudConnectError::AuthorizationDenied)
    ));

    let token = mgr
        .poll_device_token(
            &cfg,
            &prompt.device_code,
            Duration::from_millis(5),
            Duration::from_secs(5),
        )
        .await?;
    assert_eq!(token.access_token, "cli-tok-99");
    assert_eq!(polls.load(Ordering::Relaxed), 3);

    mgr.set_desired_link(true, CloudCliLinkMode::Managed)?;
    mgr.set_linked_environment("user_1", &origin)?;
    mgr.set_publish_agent_activity(true)?;
    mgr.mark_link_revoked("server_revoked")?;
    let status = mgr.read_status()?;
    assert_eq!(status.exposure, CloudConnectExposure::Managed);
    assert_eq!(status.link_state, CloudLinkProvisionState::Revoked);
    assert!(status.authenticated);
    assert!(status.publish_agent_activity);
    assert!(status.is_revoked());
    assert!(secrets.get(CLOUD_LINKED_USER_ID)?.is_some());
    assert!(secrets.get(RELAY_URL_SECRET)?.is_some());
    assert!(secrets.get(PUBLISH_AGENT_ACTIVITY_SECRET)?.is_some());
    assert!(secrets.get(CLOUD_LINK_REVOKED)?.is_some());

    let report = mgr
        .disconnect(CloudDisconnectOptions {
            clear_authorization: true,
            environment_id: Some("env-cloud-1".to_owned()),
            relay_url_override: None,
        })
        .await?;
    assert_eq!(report.relay_outcome, RelayUnlinkOutcome::Revoked);
    assert!(report.cleared_authorization);
    assert_eq!(revoked_calls.load(Ordering::Relaxed), 1);

    let after = mgr.read_status()?;
    assert_eq!(after.exposure, CloudConnectExposure::Disabled);
    assert_eq!(after.link_state, CloudLinkProvisionState::NotProvisioned);
    assert!(!after.authenticated);
    assert!(secrets.get(CLOUD_LINKED_USER_ID)?.is_none());
    assert!(secrets.get(RELAY_URL_SECRET)?.is_none());
    assert!(secrets.get(PUBLISH_AGENT_ACTIVITY_SECRET)?.is_none());
    assert!(secrets.get(CLOUD_LINK_REVOKED)?.is_none());

    let _ = stop_tx.send(true);
    Ok(())
}
