use std::error::Error;

use legion_contracts::brand::LEGION_CODE;
use legion_contracts::ids::ProviderDriverKind;
use legion_contracts::permissions::EnvironmentScope;
use legion_protocol::auth::{
    NewSessionLease, PEER_LINK_RENEWAL_STEP_MS, PEER_LINK_SESSION_TTL_MS, SessionLeaseRegistry,
    SessionVerificationError,
};
use legion_protocol::mcp::{
    canonical_mcp_tool_name, has_branded_mcp_tool_prefix, is_branded_mcp_tool, matches_mcp_header,
    mcp_tool_candidates,
};
use legion_protocol::provider_env::{
    ChannelUpdatePromptOutcome, HOMEBREW_LATEST_CASK_SWITCH_COMMAND,
    HOMEBREW_STABLE_CASK_UPDATE_COMMAND, SanitizedEnv, channel_latest_version_below,
    evaluate_channel_update_prompt, filter_npm_env, is_blocked_ambient_env_key,
    npm_prefix_single_flight_key, parse_npm_ls_versions, parse_npm_outdated_latest_versions,
    plan_npm_prefix_batch_query, provider_update_channel_lags,
};
use legion_protocol::tunnel::{
    PREVIEW_TUNNEL_DISABLED_HEADER, PREVIEW_TUNNEL_PORT_REFUSED_HEADER, PreviewTunnelStatusHeader,
    all_preview_tunnel_header_aliases, classify_preview_tunnel_headers,
    preview_tunnel_header_candidates,
};
use legion_protocol::url_redaction::{
    redact_http_request_target, redact_url, redact_url_query_tokens,
};

#[test]
fn peer_lease_touch_renews_by_step_and_refuses_revoked_or_expired() -> Result<(), Box<dyn Error>> {
    let reg = SessionLeaseRegistry::new();
    let t0 = 1_000_000_i64;
    let initial_expiry = t0 + PEER_LINK_SESSION_TTL_MS;
    reg.register(NewSessionLease {
        session_id: "peer-1".to_owned(),
        subject: "mcp-client".to_owned(),
        scopes: vec![EnvironmentScope::OrchestrationRead],
        proof_key_thumbprint: None,
        renewable: false,
        issued_at_ms: t0,
        db_expires_at_ms: initial_expiry,
        label: Some("peer".to_owned()),
    })?;

    // Within 1 day of issuance, sliding expiry delta < step -> no write/renewal.
    let early = reg.touch_active_peer_lease("peer-1", t0 + PEER_LINK_RENEWAL_STEP_MS - 1)?;
    assert!(!early.renewed);
    assert_eq!(early.lease.db_expires_at_ms, initial_expiry);

    // At >= 1 day elapsed, slides forward to now + 30d.
    let at_step = reg.touch_active_peer_lease("peer-1", t0 + PEER_LINK_RENEWAL_STEP_MS)?;
    assert!(at_step.renewed);
    assert_eq!(
        at_step.lease.db_expires_at_ms,
        t0 + PEER_LINK_RENEWAL_STEP_MS + PEER_LINK_SESSION_TTL_MS
    );

    // Non-monotonic / already longer lease never shrinks.
    let earlier_call = reg.touch_active_peer_lease("peer-1", t0 + 1_000)?;
    assert!(!earlier_call.renewed);
    assert_eq!(
        earlier_call.lease.db_expires_at_ms,
        t0 + PEER_LINK_RENEWAL_STEP_MS + PEER_LINK_SESSION_TTL_MS
    );

    // Expired touch fails cleanly.
    let expired_err = reg
        .touch_active_peer_lease("peer-1", at_step.lease.db_expires_at_ms)
        .err()
        .ok_or("expected DatabaseLeaseExpired")?;
    assert!(matches!(
        expired_err,
        SessionVerificationError::DatabaseLeaseExpired { .. }
    ));

    // Revoked peer lease refuses extension.
    reg.revoke_session("peer-1", t0 + 5_000)?;
    let revoked_err = reg
        .touch_active_peer_lease("peer-1", t0 + 6_000)
        .err()
        .ok_or("expected Revoked")?;
    assert!(matches!(
        revoked_err,
        SessionVerificationError::Revoked { .. }
    ));
    Ok(())
}

#[test]
fn url_redaction_masks_sensitive_query_and_signed_path_tokens() {
    assert_eq!(
        redact_url_query_tokens(
            "https://alice:hunter2@host.test/api/threads/1?token=s3cret&v=2&sig=abc&access_token=tok&signature=xyz#top"
        ),
        "https://[REDACTED]@host.test/api/threads/1?token=[REDACTED]&v=2&sig=[REDACTED]&access_token=[REDACTED]&signature=[REDACTED]#top"
    );
    assert_eq!(
        redact_url("/ws?Token=abc&cursor=42"),
        "/ws?Token=[REDACTED]&cursor=42"
    );
    assert_eq!(
        redact_http_request_target("/api/assets/payload.sig/image.png?v=1"),
        "/api/assets/REDACTED"
    );
    assert_eq!(
        redact_http_request_target("/api/attachments/upload/payload.sig?v=1"),
        "/api/attachments/upload/REDACTED"
    );
    assert_eq!(
        redact_http_request_target("/api/hooks/hook-9/secret-tok"),
        "/api/hooks/hook-9/REDACTED"
    );
    // Sibling prefix not ending in '/' stays unredacted.
    assert_eq!(
        redact_http_request_target("/api/assets-other/file?v=1"),
        "/api/assets-other/file?v=1"
    );
}

#[test]
fn tunnel_status_headers_cover_legion_brand_aliases() {
    assert_eq!(
        PREVIEW_TUNNEL_PORT_REFUSED_HEADER,
        "x-legioncode-tunnel-port-refused"
    );
    assert_eq!(
        PREVIEW_TUNNEL_DISABLED_HEADER,
        "x-legioncode-tunnel-disabled"
    );
    assert_eq!(
        preview_tunnel_header_candidates(&LEGION_CODE, PreviewTunnelStatusHeader::Disabled),
        vec!["x-legioncode-tunnel-disabled".to_owned()]
    );
    let aliases = all_preview_tunnel_header_aliases(PreviewTunnelStatusHeader::PortRefused);
    assert_eq!(aliases, vec!["x-legioncode-tunnel-port-refused".to_owned()]);

    assert_eq!(
        classify_preview_tunnel_headers([
            ("X-Legioncode-Tunnel-Port-Refused", "true"),
            ("x-legioncode-tunnel-disabled", "0"),
        ]),
        Some(PreviewTunnelStatusHeader::PortRefused)
    );
    assert_eq!(
        classify_preview_tunnel_headers([
            ("x-legioncode-tunnel-port-refused", "1"),
            ("X-Legioncode-Tunnel-Disabled", "1"),
        ]),
        Some(PreviewTunnelStatusHeader::Disabled)
    );
}

#[test]
fn brand_routing_for_sanitized_env_and_mcp_tools() {
    assert!(is_blocked_ambient_env_key("LEGIONCODE_HOME"));
    assert!(is_blocked_ambient_env_key("LEGIONCODE_DEV_TOKEN"));
    assert!(is_blocked_ambient_env_key("VITE_HTTP_URL"));
    assert!(!is_blocked_ambient_env_key("PATH"));

    let sanitized = SanitizedEnv::from_entries(
        [
            ("LEGIONCODE_HOME", "/tmp/l"),
            ("LEGIONCODE_TOKEN", "x"),
            ("OPENAI_API_KEY", "ok"),
        ],
        false,
    );
    assert_eq!(sanitized.get("LEGIONCODE_HOME"), None);
    assert_eq!(sanitized.get("LEGIONCODE_TOKEN"), None);
    assert_eq!(sanitized.get("OPENAI_API_KEY"), Some("ok"));

    assert_eq!(canonical_mcp_tool_name("legion_wait"), "wait");
    assert_eq!(canonical_mcp_tool_name("mcp__legion_code__wait"), "wait");
    assert_eq!(
        canonical_mcp_tool_name("mcp__legion_code__legion_spawn"),
        "spawn"
    );
    assert!(has_branded_mcp_tool_prefix("legion_wait"));
    assert!(!has_branded_mcp_tool_prefix("wait"));
    assert!(is_branded_mcp_tool("legion_submit_output", "submit_output"));
    assert!(!is_branded_mcp_tool("submit_output", "submit_output"));
    assert!(mcp_tool_candidates("wait").contains(&"legion_wait".to_owned()));
    assert!(matches_mcp_header("X-Legioncode-Mode-Limit", "mode-limit"));
    let _ = LEGION_CODE.mcp_tool_prefix;
}

#[test]
fn npm_prefix_batching_filters_env_and_plans_queries() {
    let env = filter_npm_env([
        ("PATH", "/usr/bin"),
        ("CLAUDE_CODE_SSE_PORT", "1234"),
        ("electron_run_as_node", "1"),
    ]);
    assert_eq!(env.get("PATH").map(String::as_str), Some("/usr/bin"));
    assert!(!env.contains_key("CLAUDE_CODE_SSE_PORT"));
    assert!(!env.contains_key("electron_run_as_node"));
    let key = npm_prefix_single_flight_key("npm", "/opt/node", &env);
    assert!(key.contains("/opt/node"));

    let plan = plan_npm_prefix_batch_query(
        "/opt/node",
        &["@openai/codex", "@anthropic-ai/claude-code", "opencode-ai"],
        Some(&["@openai/codex", "@anthropic-ai/claude-code"]),
    );
    let plan = plan.as_ref();
    assert!(plan.is_some());
    if let Some(plan) = plan {
        assert_eq!(
            plan.packages,
            vec![
                "@anthropic-ai/claude-code".to_owned(),
                "@openai/codex".to_owned(),
            ]
        );
        assert_eq!(
            plan.outdated_args,
            vec![
                "outdated",
                "-g",
                "--prefix",
                "/opt/node",
                "--json",
                "@anthropic-ai/claude-code",
                "@openai/codex",
            ]
        );
        let ls = plan.build_ls_args(&["@openai/codex".to_owned()]);
        assert!(ls.is_some());
    }

    let outdated = parse_npm_outdated_latest_versions(
        r#"{"@openai/codex":{"current":"0.1.0","latest":"0.2.0"}}"#,
        &["@openai/codex", "@anthropic-ai/claude-code"],
    );
    assert_eq!(
        outdated.get("@openai/codex").and_then(Option::as_deref),
        Some("0.2.0")
    );
    assert_eq!(
        outdated.get("@anthropic-ai/claude-code").cloned().flatten(),
        None
    );

    let ls = parse_npm_ls_versions(
        r#"{"dependencies":{"@anthropic-ai/claude-code":{"version":"2.1.5"}}}"#,
        &["@anthropic-ai/claude-code"],
    );
    assert_eq!(
        ls.get("@anthropic-ai/claude-code")
            .and_then(Option::as_deref),
        Some("2.1.5")
    );
}

#[test]
fn channel_update_prompt_suppression_handles_homebrew_lag() -> Result<(), Box<dyn Error>> {
    assert_eq!(
        channel_latest_version_below(Some("2.1.311"), "2.1.321"),
        Some("2.1.311")
    );
    assert_eq!(
        channel_latest_version_below(Some("2.1.330"), "2.1.321"),
        None
    );
    assert!(provider_update_channel_lags(
        Some("2.1.311"),
        &["2.1.300", "2.1.321"]
    ));

    let claude = ProviderDriverKind::new("claudeAgent")?;
    assert_eq!(
        evaluate_channel_update_prompt(
            &claude,
            true,
            Some(HOMEBREW_STABLE_CASK_UPDATE_COMMAND),
            Some("2.1.311"),
            &["2.1.321"],
        ),
        ChannelUpdatePromptOutcome::SuppressedSwitchHomebrewChannel {
            latest_version: "2.1.311".to_owned(),
            switch_command: HOMEBREW_LATEST_CASK_SWITCH_COMMAND,
        }
    );
    assert_eq!(
        evaluate_channel_update_prompt(
            &claude,
            true,
            Some("npm install -g @anthropic-ai/claude-code@latest"),
            Some("2.1.311"),
            &["2.1.321"],
        ),
        ChannelUpdatePromptOutcome::SuppressedChannelLags {
            latest_version: "2.1.311".to_owned(),
        }
    );
    assert_eq!(
        evaluate_channel_update_prompt(
            &claude,
            true,
            Some(HOMEBREW_STABLE_CASK_UPDATE_COMMAND),
            Some("2.1.330"),
            &["2.1.321"],
        ),
        ChannelUpdatePromptOutcome::OfferStandardUpdate
    );
    Ok(())
}
