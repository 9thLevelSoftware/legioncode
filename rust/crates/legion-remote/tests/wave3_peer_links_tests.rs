use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use legion_contracts::brand::ACTIVE_BRAND;
use legion_contracts::environment::{
    EnvironmentCapabilities, ExecutionEnvironmentDescriptor, PlatformArch, PlatformInfo, PlatformOs,
};
use legion_contracts::ids::EnvironmentId;
use legion_contracts::orchestration::McpClientAccess;
use legion_contracts::peer_links::{
    PeerLinkClientPairingRequest, PeerLinkPairingLinkAccess, PeerLinkThreadItemInput,
    PeerMcpClientAccess,
};
use legion_contracts::policy::RuntimeMode;
use legion_persistence::sqlite::SqliteStore;
use legion_remote::live_network::LivePeerClient;
use legion_remote::peer::client_pairing::{
    ApprovedPairingIssue, PairingIssuerCaller, PeerClientPairingRefusalReason,
    evaluate_pairing_issue,
};
use legion_remote::peer::linked_history::{
    HistoryAnchor, LinkedHistoryDecision, LinkedHistoryPageInput, LinkedHistoryWindow,
    LinkedSnapshotInput, LoadedLinkedHistory, evaluate_linked_history_page,
    evaluate_linked_snapshot_retention, keep_loaded_history_on_reconnect,
};
use legion_remote::peer::links::{
    InsertPeerLinkInput, PeerLinksService, PeerUnlinkOutcome, format_peer_unlink_outcome,
};
use legion_remote::peer::thread_relay::{PeerThreadRelayClient, refuses_before_position};
use legion_remote::secrets::FileSecretStore;
use tempfile::tempdir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

type TestResult = Result<(), Box<dyn Error>>;

async fn spawn_peer_http_stub(
    answering_env_id: &'static str,
    revoke_hits: Arc<AtomicUsize>,
    item_query_hits: Arc<AtomicUsize>,
    last_item_target: Arc<tokio::sync::Mutex<Option<String>>>,
) -> Result<(String, tokio::task::JoinHandle<()>), Box<dyn Error>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let well_known_path = format!("/.well-known/{}/environment", ACTIVE_BRAND.cli_bin_name);
    let mut desc = ExecutionEnvironmentDescriptor::builder(
        EnvironmentId::new(answering_env_id)?,
        "Stub Peer",
        PlatformInfo::new(PlatformOs::Linux, PlatformArch::X64),
        "0.1.11",
    )
    .build();
    desc.capabilities.linked_thread_read = Some(true);
    desc.capabilities.peer_link_pairing_codes = Some(true);
    desc.capabilities.transitive_peer_linking = Some(true);
    let desc_json = serde_json::to_string(&desc)?;

    let handle = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let desc_json = desc_json.clone();
            let well_known = well_known_path.clone();
            let revoke_hits = Arc::clone(&revoke_hits);
            let item_query_hits = Arc::clone(&item_query_hits);
            let last_item_target = Arc::clone(&last_item_target);
            tokio::spawn(async move {
                let mut buf = [0_u8; 4096];
                let Ok(n) = socket.read(&mut buf).await else {
                    return;
                };
                let req = String::from_utf8_lossy(&buf[..n]);
                let first_line = req.lines().next().unwrap_or("");
                let target = first_line.split_whitespace().nth(1).unwrap_or("");
                let (status, body) = if target == well_known {
                    ("200 OK", desc_json)
                } else if target == "/oauth/mcp/revoke" {
                    revoke_hits.fetch_add(1, Ordering::Relaxed);
                    ("200 OK", "{}".to_owned())
                } else if target.starts_with("/api/links/thread/item") {
                    item_query_hits.fetch_add(1, Ordering::Relaxed);
                    *last_item_target.lock().await = Some(target.to_owned());
                    (
                        "200 OK",
                        r#"{"item":{"id":"it-1"},"revision":"rev-7"}"#.to_owned(),
                    )
                } else {
                    ("404 Not Found", r#"{"error":"not found"}"#.to_owned())
                };
                let resp = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(resp.as_bytes()).await;
            });
        }
    });
    Ok((format!("http://127.0.0.1:{port}"), handle))
}

fn insert_box_link(
    service: &PeerLinksService,
    store: &SqliteStore,
    origin: String,
    token: &str,
) -> TestResult {
    service.insert(
        store.connection(),
        InsertPeerLinkInput {
            environment_id: EnvironmentId::new("env-box")?,
            label: "Box".to_owned(),
            urls: vec![origin],
            access: McpClientAccess::Auto,
            access_token: token.to_owned(),
            linked_at: "2026-11-22T00:00:00.000Z".to_owned(),
            expires_at: "2026-12-22T00:00:00.000Z".to_owned(),
        },
    )?;
    Ok(())
}

#[tokio::test]
async fn unlink_ends_remote_session_only_when_environment_id_matches() -> TestResult {
    let dir = tempdir()?;
    let store = SqliteStore::open_in_memory()?;
    let secrets = FileSecretStore::from_state_dir(dir.path())?;
    let service = PeerLinksService::new(secrets.clone());
    let client = LivePeerClient::new()?;

    let revoke_hits = Arc::new(AtomicUsize::new(0));
    let item_hits = Arc::new(AtomicUsize::new(0));
    let last_item = Arc::new(tokio::sync::Mutex::new(None));
    let (origin_ok, h1) = spawn_peer_http_stub(
        "env-box",
        Arc::clone(&revoke_hits),
        Arc::clone(&item_hits),
        Arc::clone(&last_item),
    )
    .await?;

    insert_box_link(&service, &store, origin_ok, "tok-secret-box")?;
    let res = client
        .unlink_peer_over_http(store.connection(), &secrets, "env-box")
        .await?;
    assert_eq!(
        res,
        PeerUnlinkOutcome {
            removed: true,
            ended_there: true,
        }
    );
    assert_eq!(revoke_hits.load(Ordering::Relaxed), 1);
    assert_eq!(service.list(store.connection())?.len(), 0);
    assert_eq!(
        format_peer_unlink_outcome("env-box", res),
        "Forgot env-box and ended its session there.\n"
    );

    let wrong_revoke_hits = Arc::new(AtomicUsize::new(0));
    let (origin_other, h2) = spawn_peer_http_stub(
        "env-other",
        Arc::clone(&wrong_revoke_hits),
        Arc::clone(&item_hits),
        Arc::clone(&last_item),
    )
    .await?;
    insert_box_link(&service, &store, origin_other, "tok-secret-box-2")?;
    let mismatched = client
        .unlink_peer_over_http(store.connection(), &secrets, "env-box")
        .await?;
    assert_eq!(
        mismatched,
        PeerUnlinkOutcome {
            removed: true,
            ended_there: false,
        }
    );
    assert_eq!(wrong_revoke_hits.load(Ordering::Relaxed), 0);
    assert_eq!(
        format_peer_unlink_outcome("env-box", mismatched),
        "Forgot env-box. It did not answer, so revoke it in that environment's Sharing & access to end its session there too.\n"
    );

    h1.abort();
    h2.abort();
    Ok(())
}

#[tokio::test]
async fn thread_relay_item_forwards_revision_and_refuses_t3_before_position() -> TestResult {
    let revoke_hits = Arc::new(AtomicUsize::new(0));
    let item_hits = Arc::new(AtomicUsize::new(0));
    let last_item = Arc::new(tokio::sync::Mutex::new(None));
    let (origin_ok, handle) = spawn_peer_http_stub(
        "env-box",
        revoke_hits,
        Arc::clone(&item_hits),
        Arc::clone(&last_item),
    )
    .await?;

    assert!(refuses_before_position(
        "invalid_request",
        "Invalid parameters for tool 't3_thread_read': Unknown argument \"beforePosition\". Accepted arguments: limit."
    ));
    let relay = PeerThreadRelayClient::new()?;
    let item_res = relay
        .item(
            &origin_ok,
            "Box",
            "tok-1",
            &EnvironmentCapabilities {
                linked_thread_read: Some(true),
                ..EnvironmentCapabilities::default()
            },
            &PeerLinkThreadItemInput {
                environment_id: EnvironmentId::new("env-box")?,
                thread_id: legion_contracts::ids::ThreadId::new("t-1")?,
                item_id: legion_contracts::ids::TurnItemId::new("it-1")?,
                revision: Some("rev-7".to_owned()),
            },
        )
        .await?;
    assert_eq!(item_res.data["item"]["id"], "it-1");
    assert_eq!(item_hits.load(Ordering::Relaxed), 1);
    assert_eq!(
        last_item.lock().await.as_deref(),
        Some("/api/links/thread/item?thread=t-1&item=it-1&revision=rev-7")
    );
    handle.abort();
    Ok(())
}

#[test]
fn transitive_pairing_issuer_enforces_full_access_and_formats_labels() -> TestResult {
    let caller = PairingIssuerCaller {
        is_peer_link: true,
        access: McpClientAccess::FullAccess,
        label: format!("{} · Laptop", ACTIVE_BRAND.product_name),
    };
    let denied = evaluate_pairing_issue(
        true,
        Some(RuntimeMode::Auto),
        &caller,
        &PeerLinkClientPairingRequest {
            scopes: vec!["orchestration:read".to_owned()],
            client_label: "Mobile".to_owned(),
            link: None,
        },
        &["orchestration:read"],
    );
    assert_eq!(
        denied.err().map(|e| e.reason),
        Some(PeerClientPairingRefusalReason::NotAFullAccessLink)
    );

    let approved_client = evaluate_pairing_issue(
        true,
        Some(RuntimeMode::FullAccess),
        &caller,
        &PeerLinkClientPairingRequest {
            scopes: vec!["orchestration:read".to_owned(), "admin:nope".to_owned()],
            client_label: "a".repeat(120),
            link: None,
        },
        &["orchestration:read"],
    )
    .map_err(|e| e.message)?;
    assert_eq!(
        approved_client,
        ApprovedPairingIssue::Client {
            scopes: vec!["orchestration:read".to_owned()],
            label: format!("{} · via Laptop", "a".repeat(80)),
            ttl_ms: 180_000,
        }
    );

    let approved_link = evaluate_pairing_issue(
        true,
        None,
        &caller,
        &PeerLinkClientPairingRequest {
            scopes: Vec::new(),
            client_label: "Ignored".to_owned(),
            link: Some(PeerLinkPairingLinkAccess {
                access: PeerMcpClientAccess::Auto,
            }),
        },
        &["orchestration:read"],
    )
    .map_err(|e| e.message)?;
    assert_eq!(
        approved_link,
        ApprovedPairingIssue::PeerLink {
            access: PeerMcpClientAccess::Auto,
            label: "Link · via Laptop".to_owned(),
        }
    );
    Ok(())
}

#[test]
fn linked_history_retention_backfills_across_reconnects_and_rejects_stale_pages() {
    let anchor = HistoryAnchor {
        source_thread_id: "t-1".to_owned(),
        source_item_id: "item-older".to_owned(),
    };
    let held = LoadedLinkedHistory {
        window: LinkedHistoryWindow {
            sequence: 10,
            cursor: "cur-1".to_owned(),
        },
        next_cursor: Some("cur-0".to_owned()),
        restore_through: None,
    };
    assert!(keep_loaded_history_on_reconnect(false, true, Some(held.clone())).is_some());

    let backfill = evaluate_linked_snapshot_retention(
        &LinkedSnapshotInput {
            sequence: 12,
            history_cursor: Some("cur-2".to_owned()),
            visible_items: vec![HistoryAnchor {
                source_thread_id: "t-1".to_owned(),
                source_item_id: "item-new".to_owned(),
            }],
        },
        Some(10),
        Some(&anchor),
        Some(&held),
    );
    assert!(matches!(backfill, LinkedHistoryDecision::FetchPage { .. }));

    let w = LinkedHistoryWindow {
        sequence: 12,
        cursor: "cur-2".to_owned(),
    };
    assert_eq!(
        evaluate_linked_history_page(
            &w,
            &w,
            "cur-2",
            Some("cur-2"),
            Some(&anchor),
            Some(&LinkedHistoryPageInput {
                snapshot_sequence: 12,
                has_more_history: true,
                next_cursor: Some("cur-2".to_owned()),
                combined_visible_items: Vec::new(),
            })
        ),
        LinkedHistoryDecision::MarkPageFailed
    );
    let recovered = evaluate_linked_history_page(
        &w,
        &w,
        "cur-2",
        Some("cur-2"),
        Some(&anchor),
        Some(&LinkedHistoryPageInput {
            snapshot_sequence: 12,
            has_more_history: true,
            next_cursor: Some("cur-1".to_owned()),
            combined_visible_items: vec![anchor.clone()],
        }),
    );
    assert!(matches!(
        recovered,
        LinkedHistoryDecision::Settled {
            loaded_history: Some(LoadedLinkedHistory {
                restore_through: None,
                ..
            })
        }
    ));
}
