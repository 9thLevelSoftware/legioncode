//! Integration tests for Wave 3a `OpenCode`, Pi, Grok, ACP, ACP Registry, and Usage fixes.

use std::path::Path;

use serde_json::json;
use t3_providers::acp::{
    ACP_LOADED_COMMANDS_WAIT_MS, ACP_MCP_CALL_LAUNCHER_CMD, AcpCompactionGate, AvailableCommand,
    acp_session_update_applies_live_background_mutations, evaluate_acp_compaction_gate,
    format_acp_usage_update_emission, resolve_acp_mcp_call_arguments,
};
use t3_providers::core::{
    UsageLimitSourceSnapshot, UsageSourceAccount, UsageWindow, UsageWindowKind,
    antigravity_window_limits_model, hub_pool_reset_at, hub_routed_accounts, instance_hubs,
    transcript_line_contains_any_marker,
};
use t3_providers::drivers::{
    AcpRegistryLocalFlavor, DEFAULT_VARIANT_CHOICE_ID, OpenCodeReleaseMeta, PiAsyncChildStep,
    ProviderProbeStatus, acp_registry_should_retry_with_refreshed_index,
    build_deepseek_probe_history_isolation, detect_acp_registry_local_flavor,
    grok_should_refresh_on_interval, mark_open_code_legacy, open_code_price,
    open_code_release_line, open_code_variant_descriptor, open_code_wire_variant,
    opencode_usage_error_message, pi_async_run_should_settle, reasoning_variant_label,
};

#[test]
fn opencode_pi_and_grok_fixes() {
    // 4d5bf2d557: xhigh -> Extra High
    assert_eq!(reasoning_variant_label("xhigh"), "Extra High");
    assert_eq!(reasoning_variant_label("high"), "High");

    // da5bf96303: size-scoped legacy check (8b vs 70b stay separate; Carbon replaces Argon at 2/10)
    assert_eq!(open_code_release_line("llama-3.1-8b"), "llama8b");
    assert_eq!(open_code_release_line("llama-3.3-70b"), "llama70b");
    assert_eq!(open_code_release_line("claude-opus-5-5"), "claudeopus");
    let price_2_10 = Some(open_code_price(2.0, 10.0));
    let legacy = mark_open_code_legacy(&[
        OpenCodeReleaseMeta {
            slug: "opencode/gemini-4-argon".to_owned(),
            provider_id: "opencode".to_owned(),
            model_id: "gemini-4-argon".to_owned(),
            family: Some("gemini".to_owned()),
            released_ms: Some(100),
            deprecated: false,
            price: price_2_10.clone(),
        },
        OpenCodeReleaseMeta {
            slug: "opencode/gemini-4-carbon".to_owned(),
            provider_id: "opencode".to_owned(),
            model_id: "gemini-4-carbon".to_owned(),
            family: Some("gemini".to_owned()),
            released_ms: Some(200),
            deprecated: false,
            price: price_2_10,
        },
        OpenCodeReleaseMeta {
            slug: "opencode/llama-3.1-8b".to_owned(),
            provider_id: "opencode".to_owned(),
            model_id: "llama-3.1-8b".to_owned(),
            family: Some("llama".to_owned()),
            released_ms: Some(100),
            deprecated: false,
            price: Some(open_code_price(0.05, 0.08)),
        },
        OpenCodeReleaseMeta {
            slug: "opencode/llama-3.3-70b".to_owned(),
            provider_id: "opencode".to_owned(),
            model_id: "llama-3.3-70b".to_owned(),
            family: Some("llama".to_owned()),
            released_ms: Some(200),
            deprecated: false,
            price: Some(open_code_price(0.6, 0.8)),
        },
    ]);
    assert_eq!(
        legacy,
        vec![
            ("opencode/gemini-4-argon".to_owned(), true),
            ("opencode/gemini-4-carbon".to_owned(), false),
            ("opencode/llama-3.1-8b".to_owned(), false),
            ("opencode/llama-3.3-70b".to_owned(), false),
        ]
    );

    // 39480b5c88: custom models can return to t3:default
    let custom_desc = open_code_variant_descriptor(None, &["low", "xhigh"]).expect("desc");
    assert_eq!(custom_desc.current_value, DEFAULT_VARIANT_CHOICE_ID);
    assert_eq!(custom_desc.options[0].id, DEFAULT_VARIANT_CHOICE_ID);
    assert_eq!(
        open_code_wire_variant(Some(DEFAULT_VARIANT_CHOICE_ID)),
        None
    );
    assert_eq!(open_code_wire_variant(Some("xhigh")), Some("xhigh"));

    // 9c25b71395: OpenCode error body message surfaced
    assert_eq!(
        opencode_usage_error_message(401, r#"{"message":"OpenCode login expired"}"#),
        "OpenCode login expired"
    );

    // c53ac237a0: Pi async subagent waits until all step children leave running/pending
    let running_step = [PiAsyncChildStep {
        id: Some("s1"),
        kind: Some("step"),
        agent: Some("worker"),
        state: Some("running"),
    }];
    assert!(!pi_async_run_should_settle(Some("failed"), &running_step));
    let finished_step = [PiAsyncChildStep {
        id: Some("s1"),
        kind: Some("step"),
        agent: Some("worker"),
        state: Some("complete"),
    }];
    assert!(pi_async_run_should_settle(Some("failed"), &finished_step));

    // 0a1773aec9: skip interval recheck when Grok is Ready
    assert!(!grok_should_refresh_on_interval(ProviderProbeStatus::Ready));
    assert!(grok_should_refresh_on_interval(ProviderProbeStatus::Error));
}

#[test]
fn acp_sync_powershell_registry_and_usage_routing() {
    // 6d6ca60907 + 0c921e1fdf + baf7e5b15a + 4b0dd9eb1c
    assert_eq!(ACP_LOADED_COMMANDS_WAIT_MS, 2_000);
    assert_eq!(
        evaluate_acp_compaction_gate(true, true, None),
        AcpCompactionGate::AwaitCommandsUpdate
    );
    let compact_cmd = AvailableCommand {
        name: "compact".to_owned(),
        description: "Compact session".to_owned(),
        input: None,
    };
    assert_eq!(
        evaluate_acp_compaction_gate(true, true, Some(&[compact_cmd])),
        AcpCompactionGate::Allowed
    );
    assert!(!acp_session_update_applies_live_background_mutations(
        true, true
    ));
    assert!(!acp_session_update_applies_live_background_mutations(
        false, false
    ));
    assert!(acp_session_update_applies_live_background_mutations(
        false, true
    ));

    let post_turn = format_acp_usage_update_emission(Some(1_000), Some(128_000), None, false);
    assert!(post_turn.session_scoped);
    assert!(ACP_MCP_CALL_LAUNCHER_CMD.contains("%T3_ACP_MCP_NODE%"));
    assert_eq!(
        resolve_acp_mcp_call_arguments("-", r#"{"threadId":"t1"}"#).expect("stdin json"),
        json!({ "threadId": "t1" })
    );

    // 31e31c608d + 84eb429049 + 714997c420
    let args = vec!["acp".to_owned(), "--".to_owned(), "--extra".to_owned()];
    let dsh = build_deepseek_probe_history_isolation(
        r"C:\bin\dsh.exe",
        &args,
        Path::new("/tmp/dsh-probe"),
    )
    .expect("deepseek isolation");
    assert!(dsh.patch_json.contains("session-projection-cache"));
    assert_eq!(dsh.modified_args[0], "acp");
    assert_eq!(dsh.modified_args[1], "--patch");
    assert_eq!(dsh.modified_args[3], "--");

    assert_eq!(
        detect_acp_registry_local_flavor("custom", true, r"C:\tools\factory-droid.exe"),
        AcpRegistryLocalFlavor::Droid
    );
    let glm = detect_acp_registry_local_flavor("custom", true, "/usr/bin/glm-acp-agent");
    assert_eq!(glm, AcpRegistryLocalFlavor::Glm);
    assert!(glm.reports_per_turn_prompt_usage());
    let vibe = detect_acp_registry_local_flavor("custom", true, "/usr/bin/vibe-acp");
    assert!(vibe.registers_vibe_extensions());
    assert!(acp_registry_should_retry_with_refreshed_index(
        "download_failed",
        Some("1.0.0"),
        Some("1.0.1")
    ));
    assert!(!acp_registry_should_retry_with_refreshed_index(
        "download_failed",
        Some("1.0.1"),
        Some("1.0.1")
    ));
}

#[test]
fn usage_transcript_byte_gate_and_hub_routing() {
    // abd47de64f: byte-slice marker matching even on invalid UTF-8 bytes
    let raw_line = b"\xff\xfe{\"type\":\"result\",\"usage\":{}}\n";
    assert!(transcript_line_contains_any_marker(
        raw_line,
        &[b"\"usage\"", b"\"tokenUsage\""]
    ));
    assert!(!transcript_line_contains_any_marker(
        raw_line,
        &[b"\"other\""]
    ));

    // c436409da0: hub routing & antigravity quota model slicing
    assert!(antigravity_window_limits_model(
        "gemini_argon_5h",
        Some("ag/gemini-4-argon")
    ));
    assert!(!antigravity_window_limits_model(
        "gemini_carbon_5h",
        Some("ag/gemini-4-argon")
    ));
    let src = UsageLimitSourceSnapshot {
        origin: "https://hub.example.test".to_owned(),
        accounts: vec![
            UsageSourceAccount {
                driver: "codex".to_owned(),
                email: Some("a@example.com".to_owned()),
                workspace_id: None,
                unavailable: false,
                models: Some(vec!["gpt-5.2".to_owned()]),
                windows: vec![UsageWindow {
                    id: "sess".to_owned(),
                    label: "Session".to_owned(),
                    kind: UsageWindowKind::Session,
                    used_percent: 100.0,
                    resets_at: Some("2026-11-22T00:00:00Z".to_owned()),
                }],
            },
            UsageSourceAccount {
                driver: "codex".to_owned(),
                email: Some("other@example.com".to_owned()),
                workspace_id: None,
                unavailable: false,
                models: Some(vec!["gpt-5.3".to_owned()]),
                windows: vec![],
            },
        ],
    };
    let all_sources = [src];
    let hubs = instance_hubs(Some("https://hub.example.test"), &all_sources);
    assert_eq!(hubs.len(), 1);
    let routed = hub_routed_accounts(&hubs, "codex", Some("gpt-5.2(high)"));
    assert_eq!(routed.len(), 1);
    assert_eq!(routed[0].email.as_deref(), Some("a@example.com"));
    let reset = hub_pool_reset_at(&hubs, "codex", Some("gpt-5.2(high)"), 1_795_200_000_000);
    assert_eq!(reset.as_deref(), Some("2026-11-22T00:00:00Z"));
}
