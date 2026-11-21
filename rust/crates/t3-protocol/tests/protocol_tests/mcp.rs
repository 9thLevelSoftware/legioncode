use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use t3_contracts::ids::EnvironmentId;
use t3_contracts::orchestration::{LinkOrigin, McpClientAccess, ProviderInteractionMode};
use t3_contracts::policy::RuntimeMode;
use t3_protocol::mcp::{
    DEFAULT_WAIT_TIMEOUT_MS, MAX_WAIT_TIMEOUT_MS, McpAccessError, McpGateOutcome, McpHttpRequest,
    McpSseError, ModeCeiling, PeerMcpSessionMap, PeerMcpSessionState, PeerSessionKey,
    approved_mode_ceiling, assert_not_linked, assert_same_link, clamp_wait_timeout_ms,
    evaluate_mcp_http_request, format_mode_limit, intersect_mode_ceilings, parse_mode_limit,
    parse_sse_json_rpc_response,
};
use t3_protocol::rpc_codec::RpcId;
use tokio::sync::Notify;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn mcp_http_gate_enforces_403_405_415_406() -> TestResult {
    // 1. Pre-auth Origin header rejects with 403 before method checks (even OPTIONS/empty origin).
    for (method, origin) in [
        ("POST", "https://browser.example"),
        ("OPTIONS", ""),
        ("GET", "http://localhost:3000"),
    ] {
        let req = McpHttpRequest::new(
            method,
            Some(origin),
            Some("application/json"),
            Some("application/json, text/event-stream"),
        );
        let Err(rej) = evaluate_mcp_http_request(&req) else {
            return Err("expected 403 rejection when Origin present".into());
        };
        assert_eq!(rej.status, 403);
        assert_eq!(rej.code, "forbidden_origin");
    }

    // 2. Method gate rejects GET/PUT/PATCH/OPTIONS with 405 Allow: POST, DELETE.
    for method in ["GET", "PUT", "PATCH", "OPTIONS"] {
        let req = McpHttpRequest::new(method, None, None, None);
        let Err(rej) = evaluate_mcp_http_request(&req) else {
            return Err(format!("expected 405 for {method}").into());
        };
        assert_eq!(rej.status, 405);
        assert_eq!(rej.allow, Some("POST, DELETE"));
    }

    let del_ok = McpHttpRequest::new("DELETE", None, None, None);
    assert_eq!(
        evaluate_mcp_http_request(&del_ok)?,
        McpGateOutcome::TerminateSession
    );

    let del_disabled = McpHttpRequest {
        allow_session_termination: false,
        ..McpHttpRequest::new("DELETE", None, None, None)
    };
    let Err(del_rej) = evaluate_mcp_http_request(&del_disabled) else {
        return Err("expected 405 when DELETE disabled".into());
    };
    assert_eq!(del_rej.status, 405);
    assert_eq!(del_rej.allow, Some("POST"));

    // 3. POST requires primary Content-Type: application/json -> 415 otherwise.
    for bad_ct in [None, Some("text/plain"), Some("application/problem+json")] {
        let req = McpHttpRequest::new(
            "POST",
            None,
            bad_ct,
            Some("application/json, text/event-stream"),
        );
        let Err(rej) = evaluate_mcp_http_request(&req) else {
            return Err("expected 415 rejection".into());
        };
        assert_eq!(rej.status, 415);
    }

    // 4. POST requires Accept containing BOTH application/json and text/event-stream -> 406.
    for bad_accept in [
        None,
        Some("application/json"),
        Some("text/event-stream"),
        Some("application/json, text/event-stream;q=0"),
    ] {
        let req = McpHttpRequest::new("POST", None, Some("application/json"), bad_accept);
        let Err(rej) = evaluate_mcp_http_request(&req) else {
            return Err("expected 406 rejection".into());
        };
        assert_eq!(rej.status, 406);
    }

    let valid_post = McpHttpRequest::new(
        "POST",
        None,
        Some("application/json; charset=utf-8"),
        Some("text/event-stream, application/json;q=0.9"),
    );
    assert_eq!(
        evaluate_mcp_http_request(&valid_post)?,
        McpGateOutcome::JsonRpcPost
    );
    Ok(())
}

#[test]
fn mode_limit_link_origin_and_wait_clamp() -> TestResult {
    let parsed = parse_mode_limit(" auto / plan ")?;
    assert_eq!(
        parsed,
        ModeCeiling {
            runtime_mode: RuntimeMode::Auto,
            interaction_mode: ProviderInteractionMode::Plan,
        }
    );
    assert_eq!(format_mode_limit(parsed), "auto/plan");
    assert!(matches!(
        parse_mode_limit("auto"),
        Err(McpAccessError::InvalidModeLimit { .. })
    ));
    assert!(matches!(
        parse_mode_limit("auto/plan/extra"),
        Err(McpAccessError::InvalidModeLimit { .. })
    ));

    // Monotonic narrowing cannot widen ApprovalRequired to FullAccess.
    let narrow_base = approved_mode_ceiling(Some(McpClientAccess::ReadOnly));
    let widened_attempt = parse_mode_limit("full-access/default")?;
    let intersected = intersect_mode_ceilings(narrow_base, Some(widened_attempt), None);
    assert_eq!(intersected.runtime_mode, RuntimeMode::ApprovalRequired);

    // Narrowing FullAccess -> auto/plan + managed policy auto-accept-edits settles at auto-accept-edits/plan.
    let full = approved_mode_ceiling(Some(McpClientAccess::FullAccess));
    let bounded = intersect_mode_ceilings(full, Some(parsed), Some(RuntimeMode::AutoAcceptEdits));
    assert_eq!(bounded.runtime_mode, RuntimeMode::AutoAcceptEdits);
    assert_eq!(bounded.interaction_mode, ProviderInteractionMode::Plan);

    // LinkOrigin containment checks.
    let link_a = LinkOrigin {
        session_id: "sess-a".to_owned(),
        label: "A".to_owned(),
        access: Some(McpClientAccess::Auto),
    };
    let link_b = LinkOrigin {
        session_id: "sess-b".to_owned(),
        label: "B".to_owned(),
        access: Some(McpClientAccess::Auto),
    };
    let unfenced = LinkOrigin {
        session_id: "sess-root".to_owned(),
        label: "Root".to_owned(),
        access: Some(McpClientAccess::FullAccess),
    };

    assert!(assert_same_link(Some(&link_a), Some(&link_a)).is_ok());
    assert!(matches!(
        assert_same_link(Some(&link_a), Some(&link_b)),
        Err(McpAccessError::OutsideLink { .. })
    ));
    assert!(matches!(
        assert_same_link(Some(&link_a), None),
        Err(McpAccessError::OutsideLink { .. })
    ));
    assert!(assert_same_link(Some(&unfenced), Some(&link_b)).is_ok());
    assert!(matches!(
        assert_not_linked(Some(&link_a), "settings.update"),
        Err(McpAccessError::OperationForbiddenForLinkedClient { .. })
    ));
    assert!(assert_not_linked(Some(&unfenced), "settings.update").is_ok());

    // Wait timeout bounds: default 30s, max 45s, floor 1ms.
    assert_eq!(clamp_wait_timeout_ms(None), DEFAULT_WAIT_TIMEOUT_MS);
    assert_eq!(clamp_wait_timeout_ms(Some(-10)), 1);
    assert_eq!(clamp_wait_timeout_ms(Some(0)), 1);
    assert_eq!(clamp_wait_timeout_ms(Some(12_345)), 12_345);
    assert_eq!(clamp_wait_timeout_ms(Some(120_000)), MAX_WAIT_TIMEOUT_MS);
    Ok(())
}

#[tokio::test]
async fn sse_multi_event_matching_and_single_flight_peer_init_bug_06() -> TestResult {
    let sse = concat!(
        ": heartbeat\n\n",
        "event: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\"}\n\n",
        "data: {\"jsonrpc\":\"2.0\",\"id\":\"req-1\",\"result\":{\"step\":1}}\r\n\r\n",
        "data: {\"jsonrpc\":\"2.0\",\n",
        "data: \"id\":\"req-2\",\"result\":{\"step\":2,\"ok\":true}}\n\n"
    );

    let r2 = parse_sse_json_rpc_response(sse, &RpcId::from("req-2"))?;
    assert_eq!(r2["result"]["step"], 2);
    assert_eq!(r2["result"]["ok"], true);

    assert!(matches!(
        parse_sse_json_rpc_response(sse, &RpcId::from("req-missing")),
        Err(McpSseError::MissingMatchingResponse { .. })
    ));

    // Concurrent callers racing for the same peer initialize key trigger initializer only once.
    let sessions = PeerMcpSessionMap::new();
    let key = PeerSessionKey::new(EnvironmentId::new("env-peer")?, "http://127.0.0.1:4100/mcp");
    let init_calls = Arc::new(AtomicUsize::new(0));
    let gate = Arc::new(Notify::new());
    let first_entered = Arc::new(Notify::new());

    let mut handles = Vec::new();
    for idx in 0..6 {
        let map = sessions.clone();
        let k = key.clone();
        let calls = Arc::clone(&init_calls);
        let gate_rx = Arc::clone(&gate);
        let entered_tx = Arc::clone(&first_entered);
        handles.push(tokio::spawn(async move {
            map.get_or_init(k, || async move {
                let seq = calls.fetch_add(1, Ordering::SeqCst);
                if idx < 6 && seq == 0 {
                    entered_tx.notify_one();
                    gate_rx.notified().await;
                }
                Ok::<PeerMcpSessionState, String>(PeerMcpSessionState {
                    session_id: format!("mcp-sess-{seq}"),
                    protocol_version: Some("2025-03-26".to_owned()),
                    server_info: None,
                })
            })
            .await
        }));
    }

    first_entered.notified().await;
    tokio::task::yield_now().await;
    gate.notify_waiters();

    for handle in handles {
        let state = handle.await??;
        assert_eq!(state.session_id, "mcp-sess-0");
    }
    assert_eq!(init_calls.load(Ordering::SeqCst), 1);

    // Stale invalidation guard refuses to clear a rotated session ID.
    assert!(!sessions.invalidate_if_matches(&key, "mcp-sess-stale"));
    assert!(sessions.invalidate_if_matches(&key, "mcp-sess-0"));
    assert!(sessions.is_empty());
    Ok(())
}
