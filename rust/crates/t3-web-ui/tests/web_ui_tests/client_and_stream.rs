use t3_client_core::steady_shells::{EnvironmentShellStatus, STEADY_SHELL_RELEASE_WINDOW_MS};
use t3_contracts::ids::EnvironmentId;
use t3_contracts::orchestration::StreamItem;
use t3_shared::frame_parts::{FrameOutput, FramePartAssembler, SOCKET_FRAME_PART_BYTES};
use t3_web_ui::actions::{SteadyShellBatchItem, ThreadFamilyOp, ThreadSummaryInput};
use t3_web_ui::{ConnectionStatus, WebAppAction, WebAppEngine, WebHostEffect};

type TestRes = Result<(), Box<dyn std::error::Error>>;

#[test]
fn steady_shell_coordinator_holds_and_releases_across_peers_bug_21() -> TestRes {
    let mut engine = WebAppEngine::new("en", 0);
    let env_a = EnvironmentId::new("env-a")?;
    let row_a1 = ThreadSummaryInput {
        id: "t-a1".to_owned(),
        project_id: "p1".to_owned(),
        title: "Cached A".to_owned(),
        branch: None,
        model: None,
    };
    let row_a2 = ThreadSummaryInput {
        id: "t-a2".to_owned(),
        project_id: "p1".to_owned(),
        title: "Live A".to_owned(),
        branch: None,
        model: None,
    };
    let peer_row = ThreadSummaryInput {
        id: "t-b1".to_owned(),
        project_id: "p1".to_owned(),
        title: "Cached B".to_owned(),
        branch: None,
        model: None,
    };

    let _ = engine.dispatch(WebAppAction::StepSteadyShells {
        now_ms: 0,
        inputs: vec![
            SteadyShellBatchItem {
                environment_id: "env-a".to_owned(),
                threads: Some(vec![row_a1]),
                server_sequence: Some(1),
                status: EnvironmentShellStatus::Synchronizing,
                generation: 1,
                connecting: true,
                row_actions: 0,
            },
            SteadyShellBatchItem {
                environment_id: "env-b".to_owned(),
                threads: Some(vec![peer_row.clone()]),
                server_sequence: Some(1),
                status: EnvironmentShellStatus::Synchronizing,
                generation: 1,
                connecting: true,
                row_actions: 0,
            },
        ],
    });

    let _ = engine.dispatch(WebAppAction::StepSteadyShells {
        now_ms: 500,
        inputs: vec![
            SteadyShellBatchItem {
                environment_id: "env-a".to_owned(),
                threads: Some(vec![row_a2]),
                server_sequence: Some(2),
                status: EnvironmentShellStatus::Live,
                generation: 1,
                connecting: true,
                row_actions: 0,
            },
            SteadyShellBatchItem {
                environment_id: "env-b".to_owned(),
                threads: Some(vec![peer_row]),
                server_sequence: Some(1),
                status: EnvironmentShellStatus::Synchronizing,
                generation: 1,
                connecting: true,
                row_actions: 0,
            },
        ],
    });
    assert_eq!(
        engine
            .state()
            .client
            .steady_shells
            .shown(&env_a)
            .map(|v| v[0].title.as_str()),
        Some("Cached A")
    );
    let token = engine
        .state()
        .client
        .last_steady_step
        .and_then(|s| s.timer_token);
    assert!(token.is_some());

    let _ = engine.dispatch(WebAppAction::PollSteadyShells {
        now_ms: 500 + STEADY_SHELL_RELEASE_WINDOW_MS,
        timer_token: token,
    });
    assert_eq!(
        engine
            .state()
            .client
            .steady_shells
            .shown(&env_a)
            .map(|v| v[0].title.as_str()),
        Some("Live A")
    );
    Ok(())
}

#[test]
fn thread_family_cache_tracks_generation_mount_and_ttl() -> TestRes {
    let mut engine = WebAppEngine::new("en", 0);
    for op in [
        ThreadFamilyOp::MountFamily,
        ThreadFamilyOp::SetGeneration { generation: 1 },
        ThreadFamilyOp::BeginFetch,
        ThreadFamilyOp::CompleteFetch {
            generation: 1,
            threads: vec![ThreadSummaryInput {
                id: "child-1".to_owned(),
                project_id: "p1".to_owned(),
                title: "Child thread".to_owned(),
                branch: None,
                model: None,
            }],
            snapshot_sequence: 5,
            error: None,
        },
    ] {
        engine.try_dispatch(WebAppAction::TouchThreadFamily {
            environment_id: "env-local".to_owned(),
            thread_id: "parent-1".to_owned(),
            op,
            now_ms: Some(100),
        })?;
    }
    assert_eq!(
        engine.state().client.thread_families.cached_entry_count(),
        1
    );
    engine.try_dispatch(WebAppAction::TouchThreadFamily {
        environment_id: "env-local".to_owned(),
        thread_id: "parent-1".to_owned(),
        op: ThreadFamilyOp::Release,
        now_ms: Some(200),
    })?;
    engine.try_dispatch(WebAppAction::TouchThreadFamily {
        environment_id: "env-local".to_owned(),
        thread_id: "parent-1".to_owned(),
        op: ThreadFamilyOp::EvictExpired,
        now_ms: Some(70_000),
    })?;
    assert_eq!(
        engine.state().client.thread_families.cached_entry_count(),
        0
    );
    Ok(())
}

#[test]
fn ws_frame_parts_splits_large_payload_and_reassembles_incoming_parts_bug_21() -> TestRes {
    let mut engine = WebAppEngine::new("en", 10);
    let _ = engine.dispatch(WebAppAction::CreateThread {
        project_id: "p1".to_owned(),
        title: "Large Prompt".to_owned(),
        model: None,
    });
    let huge = "🚀".repeat(16_000);
    let _ = engine.dispatch(WebAppAction::UpdateComposerDraft { text: huge.clone() });
    let fx = engine.dispatch(WebAppAction::SendMessage);
    let Some(WebHostEffect::WsSendFrameParts { parts }) = fx.first() else {
        return Err("expected WsSendFrameParts".into());
    };
    assert!(parts.len() > 1);
    assert!(parts.iter().all(|p| p.len() <= SOCKET_FRAME_PART_BYTES));

    let mut asm = FramePartAssembler::new();
    let mut joined = None;
    for part in parts {
        if let FrameOutput::Complete(whole) = asm.push(part) {
            joined = Some(whole.into_owned());
        }
    }
    assert!(joined.is_some_and(|text| text.contains(&huge)));

    let ack_fx = engine.dispatch(WebAppAction::ReceiveWsTextFrame {
        raw_frame: "\u{001e}{\"kind\":\"synch".to_owned(),
        now_ms: Some(20),
    });
    assert!(matches!(
        ack_fx.first(),
        Some(WebHostEffect::WsSendFrameParts { parts }) if parts == &vec![r#"{"_tag":"Pong"}"#.to_owned()]
    ));
    let done_fx = engine.dispatch(WebAppAction::ReceiveWsTextFrame {
        raw_frame: "ronized\"}".to_owned(),
        now_ms: Some(30),
    });
    assert_eq!(done_fx, Vec::<WebHostEffect>::new());
    assert_eq!(
        engine.state().client.connection_status,
        ConnectionStatus::Live
    );
    Ok(())
}

#[test]
fn stream_item_unknown_event_is_forward_compatible_and_non_string_type_fails_bug_18() -> TestRes {
    let mut engine = WebAppEngine::new("en", 10);
    let _ = engine.dispatch(WebAppAction::CreateThread {
        project_id: "p1".to_owned(),
        title: "T".to_owned(),
        model: None,
    });
    let tid = engine
        .state()
        .active_thread_id
        .clone()
        .ok_or("missing thread")?;
    let ok = engine.dispatch_json(&format!(
        r#"{{"type":"receiveStreamItem","threadId":"{tid}","item":{{"kind":"event","sequence":9,"event":{{"id":"e9","type":"future.event","threadId":"{tid}","occurredAt":"2026-11-20T00:00:00Z","payload":{{}}}}}}}}"#
    ))?;
    assert!(ok.contains(r#""unknownEventCount":1"#));

    let bad = engine.dispatch_json(&format!(
        r#"{{"type":"receiveStreamItem","threadId":"{tid}","item":{{"kind":"event","sequence":10,"event":{{"id":"e10","type":"turn-item.updated","threadId":"{tid}","occurredAt":"2026-11-20T00:00:00Z","payload":{{"type":123}}}}}}}}"#
    ));
    assert!(bad.is_err());
    Ok(())
}

#[test]
fn stream_domain_events_update_messages_checkpoints_plans_and_activity() -> TestRes {
    let mut engine = WebAppEngine::new("en", 10);
    let _ = engine.dispatch(WebAppAction::CreateThread {
        project_id: "p1".to_owned(),
        title: "Stream Events".to_owned(),
        model: None,
    });
    let tid = engine
        .state()
        .active_thread_id
        .clone()
        .ok_or("missing thread")?;
    let cmd_item: StreamItem = serde_json::from_str(&format!(
        r#"{{"kind":"event","sequence":2,"event":{{"id":"e2","type":"turn-item.updated","threadId":"{tid}","occurredAt":"2026-11-20T00:00:00Z","payload":{{"type":"command_execution","id":"ti-1","threadId":"{tid}","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-11-20T00:00:00Z","input":"cargo test","output":"ok","exitCode":0}}}}}}"#
    ))?;
    let _ = engine.dispatch(WebAppAction::ReceiveStreamItem {
        thread_id: Some(tid.clone()),
        item: cmd_item,
        now_ms: Some(15),
    });
    let cp_item: StreamItem = serde_json::from_str(&format!(
        r#"{{"kind":"event","sequence":3,"event":{{"id":"e3","type":"checkpoint.captured","threadId":"{tid}","occurredAt":"2026-11-20T00:00:01Z","payload":{{"id":"cp-1","threadId":"{tid}","scopeId":"sc-1","runId":null,"nodeId":"n-1","parentCheckpointId":null,"ordinalWithinScope":1,"appRunOrdinal":1,"ref":"refs/t3/cp/1","status":"ready","files":[],"capturedAt":"2026-11-20T00:00:01Z"}}}}}}"#
    ))?;
    let _ = engine.dispatch(WebAppAction::ReceiveStreamItem {
        thread_id: Some(tid),
        item: cp_item,
        now_ms: Some(20),
    });

    let html = engine.render_html();
    assert!(html.contains("cargo test [exit 0]"));
    assert!(html.contains("Checkpoint #1"));
    assert!(html.contains("refs/t3/cp/1"));
    Ok(())
}
