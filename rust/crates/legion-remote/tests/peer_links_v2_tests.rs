//! Integration tests for Peer Links v2 (`#39426`, `#40036`, `#39406`, `#39981`, `#40003`, `#39964`).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use legion_contracts::brand::{ACTIVE_BRAND, LEGION_CODE};
use legion_contracts::environment::EnvironmentCapabilities;
use legion_contracts::ids::{EnvironmentId, ThreadId, TurnItemId};
use legion_contracts::orchestration::McpClientAccess;
use legion_contracts::peer_links::{
    LINK_EXPIRES_AT_HEADER, PeerLinkClientPairingResult, PeerLinkFailureKind,
    PeerLinkLinkPairingInput, PeerLinkPairingUnsupportedReason, PeerLinkThreadHistoryInput,
    PeerLinkThreadInput, PeerLinkThreadItemInput, PeerLinkThreadReadReason, PeerMcpClientAccess,
};
use legion_persistence::sqlite::SqliteStore;
use legion_remote::live_network::LivePeerClient;
use legion_remote::peer::client_pairing::{
    BatchLinkOutcome, BatchLinkTarget, PeerPairingClient, check_transitive_pairing_capability,
    classify_batch_failure, format_transitive_pairing_label_for_brand, link_unless_linked_local,
    run_batch_linking, summarize_batch_outcomes,
};
use legion_remote::peer::links::{
    InsertPeerLinkInput, PeerLinkError, PeerLinkErrorReason, PeerLinksService, validate_peer_origin,
};
use legion_remote::peer::thread_relay::{
    NEWEST_PAGE_BEFORE_POSITION, PeerThreadRelayClient, extract_renewed_link_expiry,
    headless_linked_environments_disabled_hint_for_brand, map_relay_http_status,
    newest_page_read_args, parse_sse_stream_items, read_newest_or_retry, refuses_before_position,
};
use legion_remote::secrets::FileSecretStore;
use reqwest::header::{HeaderMap, HeaderValue};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn peer_links_batch_failure_classification_url_redaction_and_transitive_labels() -> TestResult {
    let err = validate_peer_origin("ftp://user:secret@host.test/path?tok=1").err();
    let msg = err.map_or_default(|e| e.message);
    assert!(!msg.contains("secret"));
    assert!(!msg.contains("tok=1"));
    assert_eq!(msg, "That address is not an http or https address.");

    let wrong =
        PeerLinkError::new(PeerLinkErrorReason::Unreachable, "mismatch").with_answered_as("Other");
    assert_eq!(
        classify_batch_failure(&wrong),
        PeerLinkFailureKind::WrongEnvironment
    );
    assert_eq!(
        classify_batch_failure(&PeerLinkError::new(
            PeerLinkErrorReason::PairingRejected,
            "no"
        )),
        PeerLinkFailureKind::CodeRefused
    );
    assert_eq!(
        classify_batch_failure(&PeerLinkError::new(
            PeerLinkErrorReason::NotALegionCodeEnvironment,
            "html"
        )),
        PeerLinkFailureKind::Unreachable
    );
    assert_eq!(
        format_transitive_pairing_label_for_brand(
            "LegionCode · Laptop",
            Some("Phone"),
            false,
            &ACTIVE_BRAND
        ),
        "Phone · via Laptop"
    );
    assert_eq!(
        format_transitive_pairing_label_for_brand("LegionCode · Laptop", None, true, &LEGION_CODE),
        "Link · via Laptop"
    );

    let dir = tempfile::tempdir()?;
    let store = SqliteStore::open_in_memory()?;
    let service = PeerLinksService::new(FileSecretStore::from_state_dir(dir.path())?);
    let env_id = EnvironmentId::new("env-box")?;
    let input = InsertPeerLinkInput {
        environment_id: env_id.clone(),
        label: "Box".to_owned(),
        urls: vec!["https://box.tail.ts.net".to_owned()],
        access: McpClientAccess::FullAccess,
        access_token: "tok-1".to_owned(),
        linked_at: "2026-11-20T00:00:00Z".to_owned(),
        expires_at: "2026-12-20T00:00:00Z".to_owned(),
    };
    let (first, already_1) = link_unless_linked_local(
        &service,
        store.connection(),
        "2026-11-21T00:00:00Z",
        input.clone(),
    )?;
    assert!(!already_1);
    let (second, already_2) = link_unless_linked_local(
        &service,
        store.connection(),
        "2026-11-21T00:00:00Z",
        InsertPeerLinkInput {
            access_token: "unused".to_owned(),
            ..input
        },
    )?;
    assert!(already_2);
    assert_eq!(first.urls, second.urls);

    // Transitive link pairing blocked if peer lacks `transitive_peer_linking`.
    let old_caps = EnvironmentCapabilities {
        peer_link_pairing_codes: Some(true),
        transitive_peer_linking: None,
        ..EnvironmentCapabilities::default()
    };
    assert!(matches!(
        check_transitive_pairing_capability(&second, &old_caps, true),
        Some(PeerLinkClientPairingResult::Unsupported {
            reason: PeerLinkPairingUnsupportedReason::Version,
            ..
        })
    ));
    Ok(())
}

async fn spawn_mock_peer_server()
-> Result<(String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let origin = format!("http://127.0.0.1:{port}");
    let pair_calls = Arc::new(AtomicUsize::new(0));
    let pair_calls_srv = Arc::clone(&pair_calls);

    let server_task = tokio::spawn(async move {
        for _ in 0..5 {
            let Ok((mut sock, _)) = listener.accept().await else {
                break;
            };
            let pair_calls_req = Arc::clone(&pair_calls_srv);
            tokio::spawn(async move {
                let mut buf = [0_u8; 4096];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let line = req.lines().next().unwrap_or("");
                let (status, hdrs, body) = if line
                    .starts_with("GET /.well-known/legioncode/environment")
                {
                    let desc = json!({
                        "environmentId": "env-remote",
                        "label": "RemoteBox",
                        "platform": { "os": "linux", "arch": "x64" },
                        "serverVersion": "0.1.11",
                        "capabilities": { "mcpModeLimitHeader": true, "linkFence": true, "linkedThreadRead": true }
                    }).to_string();
                    ("200 OK", "", desc)
                } else if line.starts_with("POST /api/auth/pair") {
                    pair_calls_req.fetch_add(1, Ordering::Relaxed);
                    (
                        "200 OK",
                        "",
                        r#"{"accessToken":"tok-abc","expiresIn":3600}"#.to_owned(),
                    )
                } else if line.starts_with("POST /api/links/client-pairing") {
                    (
                        "200 OK",
                        "",
                        r#"{"credential":"code-123","expiresAt":"2026-12-01T00:00:00Z"}"#
                            .to_owned(),
                    )
                } else if line.starts_with("GET /api/links/thread/stream") {
                    (
                        "200 OK",
                        "x-legioncode-link-expires-at: 2026-12-30T00:00:00Z\r\n",
                        "data: {\"kind\":\"snapshot\"}\n\ndata: {\"kind\":\"unknown-event\"}\n\n"
                            .to_owned(),
                    )
                } else {
                    (
                        "404 Not Found",
                        "",
                        r#"{"error":"thread_not_found"}"#.to_owned(),
                    )
                };
                let resp = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n{hdrs}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            });
        }
    });
    Ok((origin, pair_calls, server_task))
}

async fn assert_thread_relay_roundtrip(
    origin: &str,
    env_id: &EnvironmentId,
    caps: &EnvironmentCapabilities,
) -> TestResult {
    let relay = PeerThreadRelayClient::new()?;
    let stream = relay
        .stream_snapshot(
            origin,
            "RemoteBox",
            "tok-abc",
            caps,
            &PeerLinkThreadInput {
                environment_id: env_id.clone(),
                thread_id: ThreadId::new("thread-1")?,
            },
        )
        .await?;
    assert_eq!(stream.data, vec![json!({ "kind": "snapshot" })]);
    assert_eq!(
        stream.renewed_expires_at.as_deref(),
        Some("2026-12-30T00:00:00Z")
    );

    let missing = relay
        .history_page(
            origin,
            "RemoteBox",
            "tok-abc",
            caps,
            &PeerLinkThreadHistoryInput {
                environment_id: env_id.clone(),
                thread_id: ThreadId::new("thread-missing")?,
                cursor: "c0".to_owned(),
            },
        )
        .await;
    assert!(matches!(missing, Err(e) if e.reason == PeerLinkThreadReadReason::ThreadNotFound));
    let _ = relay
        .item(
            origin,
            "RemoteBox",
            "tok-abc",
            caps,
            &PeerLinkThreadItemInput {
                environment_id: env_id.clone(),
                thread_id: ThreadId::new("thread-1")?,
                item_id: TurnItemId::new("item-1")?,
                revision: None,
            },
        )
        .await;
    Ok(())
}

#[tokio::test]
async fn batch_linking_and_transitive_pairing_and_thread_relay_over_http() -> TestResult {
    let (origin, pair_calls, server_task) = spawn_mock_peer_server().await?;

    let dir = tempfile::tempdir()?;
    let db = SqliteStore::open_in_memory()?;
    let secrets = FileSecretStore::from_state_dir(dir.path())?;
    let client = LivePeerClient::new()?;
    let targets = vec![BatchLinkTarget {
        environment_id: EnvironmentId::new("env-remote")?,
        label: "RemoteBox".to_owned(),
        url: origin.clone(),
        alternate_urls: vec![],
        pairing_code: "pair-1".to_owned(),
        access: McpClientAccess::FullAccess,
    }];
    let first = run_batch_linking(
        &client,
        db.connection(),
        &secrets,
        &targets,
        "Client",
        None,
        "2026-11-21T00:00:00Z",
    )
    .await;
    let second = run_batch_linking(
        &client,
        db.connection(),
        &secrets,
        &targets,
        "Client",
        None,
        "2026-11-21T00:00:00Z",
    )
    .await;
    assert_eq!(pair_calls.load(Ordering::Relaxed), 1);
    assert_eq!(summarize_batch_outcomes(&first).newly_linked, 1);
    assert_eq!(summarize_batch_outcomes(&second).already_linked, 1);
    let BatchLinkOutcome::Linked { link, .. } = &second[0] else {
        return Err("expected linked outcome".into());
    };

    let caps = EnvironmentCapabilities {
        peer_link_pairing_codes: Some(true),
        transitive_peer_linking: Some(true),
        linked_thread_read: Some(true),
        ..EnvironmentCapabilities::default()
    };
    let pairing = PeerPairingClient::new()?;
    let res = pairing
        .request_link_pairing(
            link,
            &caps,
            &origin,
            "tok-abc",
            "Laptop",
            &PeerLinkLinkPairingInput {
                environment_id: link.environment_id.clone(),
                access: PeerMcpClientAccess::Auto,
            },
        )
        .await?;
    assert!(
        matches!(res, PeerLinkClientPairingResult::Credential { credential, .. } if credential == "code-123")
    );

    assert_thread_relay_roundtrip(&origin, &link.environment_id, &caps).await?;
    server_task.abort();
    Ok(())
}

#[tokio::test]
async fn thread_relay_newest_page_retry_and_headless_hint() -> TestResult {
    assert!(
        headless_linked_environments_disabled_hint_for_brand(&LEGION_CODE)
            .contains("~/.legioncode by default")
    );
    let refused = map_relay_http_status(403, Some("linked_environments_off"), "Box");
    assert_eq!(refused.reason, PeerLinkThreadReadReason::Rejected);
    assert!(refused.message.contains("linkedEnvironmentsEnabled"));

    let mut hdrs = HeaderMap::new();
    hdrs.insert(
        LINK_EXPIRES_AT_HEADER,
        HeaderValue::from_static("2026-12-01T00:00:00Z"),
    );
    assert_eq!(
        extract_renewed_link_expiry(&hdrs).as_deref(),
        Some("2026-12-01T00:00:00Z")
    );
    assert_eq!(
        parse_sse_stream_items(
            "data: {\"kind\":\"unknown-event\"}\n\ndata: {\"kind\":\"turn\"}\n\n"
        ),
        vec![json!({"kind":"turn"})]
    );

    assert_eq!(
        newest_page_read_args(&json!({"threadId":"t1","limit":10}), true),
        Some(json!({"threadId":"t1","limit":10,"beforePosition":NEWEST_PAGE_BEFORE_POSITION}))
    );
    assert_eq!(
        newest_page_read_args(&json!({"threadId":"t1","afterPosition":-1}), true),
        None
    );
    assert!(refuses_before_position(
        "invalid_request",
        "Invalid parameters for tool 'legion_thread_read': Unknown argument \"beforePosition\". Accepted arguments: limit."
    ));
    assert!(!refuses_before_position(
        "thread_not_found",
        "Unknown argument \"beforePosition\". Accepted arguments: x."
    ));

    let attempts = Arc::new(AtomicUsize::new(0));
    let retried = read_newest_or_retry(json!({"threadId":"t1"}), true, |args| {
        let count = Arc::clone(&attempts);
        async move {
            count.fetch_add(1, Ordering::Relaxed);
            if args.get("beforePosition").is_some() {
                Err(("invalid_request".to_owned(), "Invalid parameters for tool 'legion_thread_read': Unknown argument \"beforePosition\". Accepted arguments: limit.".to_owned()))
            } else {
                Ok("legacy-page")
            }
        }
    }).await;
    assert_eq!(retried, Ok("legacy-page"));
    assert_eq!(attempts.load(Ordering::Relaxed), 2);
    Ok(())
}
