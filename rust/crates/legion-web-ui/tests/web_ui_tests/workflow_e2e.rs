use legion_contracts::orchestration::StreamItem;
use legion_web_ui::actions::ProjectInput;
use legion_web_ui::{ConnectionStatus, WebAppAction, WebAppEngine, WebHostEffect, web_ui_version};

type TestRes = Result<(), Box<dyn std::error::Error>>;

fn sample_snapshot_json() -> &'static str {
    r#"{
        "kind":"snapshot",
        "snapshotSequence":4,
        "projection":{
            "thread":{
                "createdBy":"user","creationSource":"web","id":"thread-1","projectId":"proj-1",
                "title":"Fix login flow","providerInstanceId":"codex",
                "modelSelection":{"instanceId":"codex","model":"gpt-5"},
                "runtimeMode":"approval-required","interactionMode":"default",
                "branch":"fix/login","worktreePath":null,"activeProviderThreadId":null,
                "lineage":{"parentThreadId":null,"relationshipToParent":null,"rootThreadId":"thread-1"},
                "forkedFrom":null,"createdAt":"2026-11-20T00:00:00Z","updatedAt":"2026-11-20T00:00:00Z",
                "archivedAt":null,"deletedAt":null
            },
            "runs":[],"attempts":[],"nodes":[],"subagents":[],"providerSessions":[],
            "providerThreads":[],"providerTurns":[],"runtimeRequests":[],
            "messages":[{
                "createdBy":"agent","creationSource":"web","id":"m-1","threadId":"thread-1",
                "runId":null,"nodeId":null,"role":"assistant","text":"Checking login handler.",
                "attachments":[],"streaming":false,"createdAt":"2026-11-20T00:00:01Z","updatedAt":"2026-11-20T00:00:01Z"
            }],
            "plans":[],"turnItems":[],"checkpointScopes":[],"checkpoints":[],
            "contextHandoffs":[],"contextTransfers":[],"visibleTurnItems":[],"updatedAt":"2026-11-20T00:00:01Z"
        }
    }"#
}

#[test]
fn pairing_project_list_thread_stream_and_send_message_e2e() -> TestRes {
    assert_ne!(web_ui_version(), "");
    let mut engine = WebAppEngine::new("en-US", 1_000);
    assert!(
        engine
            .render_html()
            .contains(r#"data-testid="pairing-form""#)
    );

    let _ = engine.dispatch(WebAppAction::SetPairingInput {
        value: "pair-secret-123".to_owned(),
    });
    let pair_fx = engine.dispatch(WebAppAction::SubmitPairing {
        client_label: Some("e2e-browser".to_owned()),
    });
    assert_eq!(pair_fx.len(), 1);
    assert!(matches!(
        &pair_fx[0],
        WebHostEffect::HttpFetch { method, url, body: Some(_), bearer_token: None }
        if method == "POST" && url == "/api/auth/pair"
    ));

    let paired_fx = engine.dispatch(WebAppAction::ApplyPairingSuccess {
        access_token: "tok-live-abc".to_owned(),
        session_id: Some("sess-1".to_owned()),
        role: Some("admin".to_owned()),
        scopes: vec!["*".to_owned()],
    });
    assert_eq!(paired_fx.len(), 3);
    assert!(matches!(
        &paired_fx[0],
        WebHostEffect::PersistSessionToken { token } if token == "tok-live-abc"
    ));
    assert!(matches!(
        &paired_fx[1],
        WebHostEffect::HttpFetch { method, url, bearer_token: Some(tok), .. }
        if method == "GET" && url == "/api/projects" && tok == "tok-live-abc"
    ));
    assert!(
        matches!(&paired_fx[2], WebHostEffect::WsConnect { url } if url.contains("frameParts=1"))
    );

    let create_proj_fx = engine.dispatch(WebAppAction::CreateProject {
        workspace_root: "/workspace/demo".to_owned(),
        title: Some("Demo Repo".to_owned()),
    });
    assert!(matches!(
        &create_proj_fx[0],
        WebHostEffect::HttpFetch { method, url, bearer_token: Some(tok), .. }
        if method == "POST" && url == "/api/projects/mutate" && tok == "tok-live-abc"
    ));

    let _ = engine.dispatch(WebAppAction::ApplyProjectsSnapshot {
        projects: vec![ProjectInput {
            id: "proj-1".to_owned(),
            title: "Demo Repo".to_owned(),
            workspace_root: "/workspace/demo".to_owned(),
            pinned_at: None,
            archived_at: None,
            updated_at: "2026-11-20T00:00:00Z".to_owned(),
        }],
    });
    let ws_fx = engine.dispatch(WebAppAction::WsConnected { now_ms: 1_500 });
    assert!(matches!(&ws_fx[0], WebHostEffect::WsSendFrameParts { parts } if !parts.is_empty()));

    let snap_item: StreamItem = serde_json::from_str(sample_snapshot_json())?;
    let _ = engine.dispatch(WebAppAction::ReceiveStreamItem {
        thread_id: Some("thread-1".to_owned()),
        item: snap_item,
        now_ms: Some(1_600),
    });
    let _ = engine.dispatch(WebAppAction::ReceiveStreamItem {
        thread_id: Some("thread-1".to_owned()),
        item: StreamItem::Synchronized,
        now_ms: Some(1_700),
    });

    let _ = engine.dispatch(WebAppAction::UpdateComposerDraft {
        text: "Run cargo test now".to_owned(),
    });
    let send_fx = engine.dispatch(WebAppAction::SendMessage);
    assert!(matches!(&send_fx[0], WebHostEffect::WsSendFrameParts { parts } if !parts.is_empty()));
    assert!(matches!(
        &send_fx[1],
        WebHostEffect::FocusSelector { selector } if selector == r#"[data-testid="composer-input"]"#
    ));

    let html = engine.render_html();
    assert!(html.contains("Demo Repo"));
    assert!(html.contains("Fix login flow"));
    assert!(html.contains("Checking login handler."));
    assert!(html.contains("Run cargo test now"));
    assert!(html.contains(r#"data-testid="pairing-status""#));
    Ok(())
}

#[test]
fn sign_out_and_restore_session_token_round_trip() {
    let mut engine = WebAppEngine::new("en", 100);
    let restore_fx = engine.dispatch(WebAppAction::RestoreSessionToken {
        token: "persisted-bearer".to_owned(),
    });
    assert!(engine.state().auth.paired);
    assert_eq!(restore_fx.len(), 2);

    let out_fx = engine.dispatch(WebAppAction::SignOut);
    assert!(!engine.state().auth.paired);
    assert_eq!(
        out_fx,
        vec![WebHostEffect::PersistSessionToken {
            token: String::new()
        }]
    );
}

#[test]
fn reconnect_backoff_tracks_session_loss_and_approval_pending() {
    let mut engine = WebAppEngine::new("en", 1_000);
    let _ = engine.dispatch(WebAppAction::WsConnected { now_ms: 1_000 });
    let _ = engine.dispatch(WebAppAction::WsDisconnected {
        now_ms: 1_200,
        approval_pending: false,
        random: 0.0,
    });
    assert_eq!(
        engine.state().client.connection_status,
        ConnectionStatus::Disconnected
    );
    assert_eq!(
        engine.state().client.last_backoff.map(|d| d.delay_ms),
        Some(125)
    );

    let _ = engine.dispatch(WebAppAction::WsDisconnected {
        now_ms: 1_400,
        approval_pending: true,
        random: 0.5,
    });
    assert_eq!(
        engine.state().client.connection_status,
        ConnectionStatus::AwaitingApproval
    );
    assert_eq!(
        engine.state().client.last_backoff.map(|d| d.delay_ms),
        Some(4_000)
    );
}
