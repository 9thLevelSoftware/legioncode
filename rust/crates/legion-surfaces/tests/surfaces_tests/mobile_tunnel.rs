//! BUG-26 integration tests: per-pane Ghostty theme storage, HTTP-header-only host swapping
//! across chunk splits, and per-WebView tunnel registry isolation.

use legion_surfaces::mobile_tunnel::{
    HttpHeaderHostSwap, HttpProxyPhase, ScopedWebViewTunnelRegistry, TerminalThemeFileStore,
    TunnelRegistryError, render_ghostty_theme_config,
};

#[test]
fn terminal_theme_store_prevents_multi_pane_clobbering_and_rotates_on_change() {
    let mut store = TerminalThemeFileStore::new();
    let cfg = render_ghostty_theme_config("background = #111111", 14.0, Some("dark"));

    let p1 = store.upsert("pane/left:1", &cfg);
    let p2 = store.upsert("pane/right:2", &cfg);

    assert!(p1.changed);
    assert!(p2.changed);
    assert_ne!(p1.filename, p2.filename);
    assert!(p1.filename.starts_with("legioncode-ghostty-pane-left-1-"));
    assert!(p2.filename.starts_with("legioncode-ghostty-pane-right-2-"));
    assert_eq!(
        std::path::Path::new(&p1.filename)
            .extension()
            .and_then(|ext| ext.to_str()),
        Some("conf")
    );
    assert_eq!(store.pane_count(), 2);

    // Re-upserting identical content for pane 1 is a no-op with no obsolete file.
    let same = store.upsert("pane/left:1", &cfg);
    assert!(!same.changed);
    assert_eq!(same.obsolete_filename, None);

    // Updating pane 1 leaves pane 2 intact and schedules pane 1's old file for removal.
    let updated_cfg = render_ghostty_theme_config("background = #ffffff", 16.0, Some("light"));
    let rotated = store.upsert("pane/left:1", updated_cfg);
    assert!(rotated.changed);
    assert_eq!(
        rotated.obsolete_filename.as_deref(),
        Some(p1.filename.as_str())
    );
    assert_ne!(rotated.filename, p2.filename);
    assert_eq!(
        store.get("pane/right:2").map(|e| e.filename.as_str()),
        Some(p2.filename.as_str())
    );
    assert!(store.remove("pane/left:1").is_some());
    assert_eq!(store.pane_count(), 1);
}

#[test]
fn http_header_host_swap_handles_byte_split_crlf_and_never_corrupts_json_body() {
    let raw_request = concat!(
        "POST /api/proxy HTTP/1.1\r\n",
        "hOsT: LCLO.TEST:4443\r\n",
        "X-Forwarded-Host: lclo.test\r\n",
        "Content-Type: application/json\r\n",
        "\r\n",
        "{\"prompt\":\"Host: lclo.test\",\"secondary\":\"\\r\\nHost: lclo.test:4443\"}",
    );

    let mut swap = HttpHeaderHostSwap::new("lclo.test", "127.0.0.1");
    let mut forwarded = Vec::new();
    let mut transitioned_at_byte: Option<usize> = None;
    let body_start = raw_request.find("\r\n\r\n").unwrap_or(raw_request.len()) + 4;

    for (idx, byte) in raw_request.as_bytes().iter().enumerate() {
        forwarded.extend_from_slice(&swap.push(&[*byte]));
        if swap.phase() == HttpProxyPhase::BodyPassthrough && transitioned_at_byte.is_none() {
            transitioned_at_byte = Some(idx + 1);
        }
    }
    // Feed an additional body chunk after transition; must remain passthrough untouched.
    let extra_body = b"\r\nHost: lclo.test\r\n";
    forwarded.extend_from_slice(&swap.push(extra_body));
    forwarded.extend_from_slice(&swap.finish());

    assert_eq!(transitioned_at_byte, Some(body_start));
    assert_eq!(swap.phase(), HttpProxyPhase::BodyPassthrough);

    let text = String::from_utf8_lossy(&forwarded);
    assert!(text.contains("hOsT: 127.0.0.1:4443\r\n"));
    assert!(text.contains("X-Forwarded-Host: lclo.test\r\n"));
    assert!(
        text.contains(r#"{"prompt":"Host: lclo.test","secondary":"\r\nHost: lclo.test:4443"}"#)
    );
    assert!(text.ends_with("\r\nHost: lclo.test\r\n"));
}

#[test]
fn http_header_host_swap_flushes_incomplete_headers_on_finish() {
    let mut swap = HttpHeaderHostSwap::new("lclo.test", "localhost");
    assert_eq!(
        swap.push(b"GET / HTTP/1.1\r\nHost: lclo.test\r\n"),
        Vec::<u8>::new()
    );
    assert_eq!(swap.phase(), HttpProxyPhase::HeaderPhase);
    let flushed = swap.finish();
    assert_eq!(flushed, b"GET / HTTP/1.1\r\nHost: lclo.test\r\n");
    assert_eq!(swap.phase(), HttpProxyPhase::BodyPassthrough);
}

#[test]
fn scoped_webview_tunnel_registry_isolates_multiple_webviews() -> Result<(), String> {
    let mut reg = ScopedWebViewTunnelRegistry::new();

    assert_eq!(
        reg.bind("", "env-1", "127.0.0.1", 1080, "lclo.test", None),
        Err(TunnelRegistryError::EmptyWebViewId)
    );
    assert_eq!(
        reg.bind("wv-1", "env-1", "127.0.0.1", 0, "lclo.test", None),
        Err(TunnelRegistryError::ZeroProxyPort)
    );

    let b1 = reg
        .bind(
            "wv-1",
            "env-a",
            "127.0.0.1",
            41001,
            "lclo.test",
            Some("https://t1.trycloudflare.com"),
        )
        .map_err(|e| e.to_string())?;
    let b2 = reg
        .bind(
            "wv-2",
            "env-b",
            "127.0.0.1",
            41002,
            "lclo.test",
            Some("https://t2.trycloudflare.com"),
        )
        .map_err(|e| e.to_string())?;
    assert_eq!(reg.active_count(), 2);
    assert!(reg.has_other_active("wv-1"));

    // Updating wv-1 URL preserves wv-2 untouched and stays on b1.generation.
    let updated = reg
        .update_tunnel_url("wv-1", Some("https://t1-new.trycloudflare.com"))
        .map_err(|e| e.to_string())?;
    assert_eq!(updated.generation, b1.generation);
    assert_eq!(reg.get("wv-2").ok_or("wv-2 missing")?, &b2);
    assert_eq!(reg.bindings_for_environment("env-b").len(), 1);

    // Re-binding wv-1 advances generation so a stale async teardown cannot remove the new session.
    let rebound = reg
        .bind(
            "wv-1",
            "env-a",
            "127.0.0.1",
            41003,
            "lclo.test",
            Some("https://t1-rebound.trycloudflare.com"),
        )
        .map_err(|e| e.to_string())?;
    assert!(rebound.generation > b1.generation);
    assert!(reg.unbind_if_generation("wv-1", b1.generation).is_none());
    assert_eq!(reg.active_count(), 2);
    assert!(
        reg.unbind_if_generation("wv-1", rebound.generation)
            .is_some()
    );
    assert!(!reg.has_other_active("wv-2"));
    assert_eq!(reg.active_count(), 1);
    Ok(())
}

#[test]
fn preview_tunnel_policy_gating_distinguishes_disabled_and_port_refused() {
    use legion_contracts::policy::{ManagedPolicySummary, ManagedPolicySummaryLimits};
    use legion_surfaces::mobile_tunnel::{
        TunnelBindSpec, TunnelNavigationState, classify_tunnel_http_refusal,
        evaluate_tunnel_navigation,
    };

    assert_eq!(
        evaluate_tunnel_navigation(true, None),
        TunnelNavigationState::Allowed
    );
    assert_eq!(
        evaluate_tunnel_navigation(false, None),
        TunnelNavigationState::TunnelDisabled
    );

    let denied_policy = ManagedPolicySummary {
        organization: "Acme Corp".to_owned(),
        support_url: None,
        locked_settings: Vec::new(),
        limits: Some(ManagedPolicySummaryLimits {
            preview_tunnel: Some(false),
            ..Default::default()
        }),
    };
    assert_eq!(
        evaluate_tunnel_navigation(true, Some(&denied_policy)),
        TunnelNavigationState::TunnelDisabled
    );

    assert_eq!(
        classify_tunnel_http_refusal(503, [("X-Legioncode-Tunnel-Disabled", "1")]),
        Some(TunnelNavigationState::TunnelDisabled)
    );
    assert_eq!(
        classify_tunnel_http_refusal(502, [("x-legioncode-tunnel-port-refused", "true")]),
        Some(TunnelNavigationState::PortRefused)
    );
    assert_eq!(
        classify_tunnel_http_refusal(502, []),
        Some(TunnelNavigationState::Unreachable)
    );

    let mut reg = ScopedWebViewTunnelRegistry::new();
    let spec = TunnelBindSpec {
        webview_id: "wv-policy",
        environment_id: "env-a",
        proxy_host: "127.0.0.1",
        proxy_port: 41005,
        stand_in_host: "lclo.test",
        tunnel_base_url: None,
    };
    assert_eq!(
        reg.bind_if_allowed(&spec, true, Some(&denied_policy)),
        Err(TunnelRegistryError::TunnelDisabled)
    );
    assert_eq!(
        reg.bind_if_allowed(&spec, false, None),
        Err(TunnelRegistryError::TunnelDisabled)
    );
    assert!(reg.bind_if_allowed(&spec, true, None).is_ok());
}
