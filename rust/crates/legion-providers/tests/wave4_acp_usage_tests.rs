//! Wave 4 upstream alignment tests covering ACP, `OpenCode`, Pi, Cursor,
//! fallback selection, PATH refresh, and windowed usage warnings.

use std::collections::BTreeMap;

use legion_providers::acp::{
    ACP_STARTUP_HANDSHAKE_TIMEOUT_MS, AcpAgentModeChange, acp_interrupt_cause_should_settle_turn,
    acp_late_session_update_thread_status, acp_startup_timeout_error_detail,
    detect_acp_agent_mode_changes, session_update_is_replay, sync_mode_into_config_options,
};
use legion_providers::core::{
    CONVENTIONAL_COMMITS_CHANGE_REQUEST_INSTRUCTIONS, PrContentPromptInput,
    TextGenerationFallbackCandidate, TextGenerationPrompts, UsageDateWindow,
    UsageLimitsUnavailable, UsageLimitsUnavailableReason, WindowedUsageRecordRef,
    collect_windowed_deduped_usage, decode_cached_malformed_diagnostics,
    filter_cached_file_for_window, format_usage_limit_notice, select_text_generation_fallback,
};
use legion_providers::drivers::{
    OpenCodeCommandCompletionStatus, OpenCodeSessionActivity, build_acp_terminal_invocation,
    cursor_turn_prompt_dropped, did_host_path_change, git_for_windows_bash,
    instances_needing_rebuild_after_path_change, is_delivered_provider_turn,
    list_login_shell_candidates, merge_path_entries, opencode_should_complete_command_immediately,
};
use legion_providers::non_acp::{
    NonAcpTurnUpdate, parse_pi_json_line, parse_pi_turn_token_usage,
    pi_token_usage_from_session_stats,
};
use serde_json::json;

#[test]
fn acp_handshake_replay_modes_and_terminal_work() {
    assert_eq!(ACP_STARTUP_HANDSHAKE_TIMEOUT_MS, 90_000);
    assert!(acp_startup_timeout_error_detail("initialize").contains("timed out waiting"));
    assert!(session_update_is_replay(
        &json!({"_meta": {"isReplay": true}})
    ));
    assert_eq!(
        acp_late_session_update_thread_status(false, "active"),
        "idle"
    );
    assert!(acp_interrupt_cause_should_settle_turn(true));

    let mut cfg =
        vec![json!({"id": "mode", "category": "mode", "type": "select", "currentValue": "plan"})];
    assert!(sync_mode_into_config_options(&mut cfg, "code"));
    let configured = BTreeMap::from([("mode".to_owned(), "plan".to_owned())]);
    let live = BTreeMap::from([("mode".to_owned(), "code".to_owned())]);
    assert_eq!(
        detect_acp_agent_mode_changes(&configured, &live, &[("mode", "plan")]),
        vec![AcpAgentModeChange {
            id: "mode".to_owned(),
            from: "plan".to_owned(),
            to: "code".to_owned(),
        }]
    );

    let bash = git_for_windows_bash(r"C:\Program Files\Git\cmd\git.exe", |p| {
        p == r"C:\Program Files\Git\bin\bash.exe"
    });
    assert_eq!(bash.as_deref(), Some(r"C:\Program Files\Git\bin\bash.exe"));
    let term = build_acp_terminal_invocation("echo hi", &[], true, bash.as_deref());
    assert!(!term.use_default_shell);
    assert_eq!(term.args[0], "-c");
    assert!(term.args[1].starts_with("chcp.com 65001"));
}

#[test]
fn opencode_pi_cursor_and_usage_notices_work() {
    assert_eq!(
        opencode_should_complete_command_immediately(
            true,
            OpenCodeSessionActivity::IdleNoActivity,
            false,
        ),
        OpenCodeCommandCompletionStatus::CompleteImmediatelyCompleted
    );
    assert_eq!(
        opencode_should_complete_command_immediately(
            true,
            OpenCodeSessionActivity::PostBoundaryMessageObserved,
            false,
        ),
        OpenCodeCommandCompletionStatus::WaitForExecutionEvents
    );

    let cc_prompt = TextGenerationPrompts::build_pr_content_prompt_with_instructions(
        &PrContentPromptInput {
            base_branch: "main".to_owned(),
            head_branch: "feat/x".to_owned(),
            commit_summary: "feat(web): add recorder".to_owned(),
            diff_summary: "1 file changed".to_owned(),
        },
        Some(CONVENTIONAL_COMMITS_CHANGE_REQUEST_INSTRUCTIONS),
    );
    assert!(
        cc_prompt
            .system
            .contains("write the title as a Conventional Commit subject too")
    );

    assert_eq!(
        format_usage_limit_notice(
            "Antigravity",
            Some(&UsageLimitsUnavailable {
                reason: UsageLimitsUnavailableReason::Unsupported,
                message: Some("Sign in with Google to see Antigravity limits.".to_owned()),
            }),
            false,
        ),
        Some("Antigravity: Sign in with Google to see Antigravity limits.".to_owned())
    );
    assert_eq!(
        format_usage_limit_notice(
            "Cursor",
            Some(&UsageLimitsUnavailable {
                reason: UsageLimitsUnavailableReason::Unsupported,
                message: None,
            }),
            false,
        ),
        None
    );

    let pi_usage = parse_pi_turn_token_usage(
        &json!({"input": 6, "cacheRead": 40_101, "cacheWrite": 10_561, "output": 123}),
    );
    assert_eq!(pi_usage.input_tokens, 50_668);
    assert_eq!(pi_usage.cached_input_tokens, 40_101);
    let pi_ev = parse_pi_json_line(
        r#"{"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"ok"},"usage":{"input":6,"cacheRead":40101,"cacheWrite":10561,"output":123}}"#,
    )
    .expect("parse pi");
    assert_eq!(
        pi_ev[1],
        NonAcpTurnUpdate::UsageReported {
            input_tokens: 50_668,
            output_tokens: 123
        }
    );
    assert_eq!(
        pi_token_usage_from_session_stats(
            &json!({"contextUsage": {"tokens": 50_791, "contextWindow": 1_000_000}}),
            None,
            Some(pi_usage),
        ),
        Some((50_791, 1_000_000, Some(pi_usage)))
    );

    assert!(cursor_turn_prompt_dropped("failed", false, true, false));
    assert!(!cursor_turn_prompt_dropped("failed", false, true, true));
    assert!(!is_delivered_provider_turn(true));
}

#[test]
fn fallback_path_refresh_and_windowed_usage_work() {
    assert_eq!(
        select_text_generation_fallback(
            &[
                TextGenerationFallbackCandidate {
                    instance_id: "inst-b",
                    driver: "claudeAgent",
                    model_slug: "image-only",
                    supports_text_generation: Some(false),
                    usable: true,
                },
                TextGenerationFallbackCandidate {
                    instance_id: "inst-b",
                    driver: "claudeAgent",
                    model_slug: "claude-haiku-4-5",
                    supports_text_generation: Some(true),
                    usable: true,
                },
            ],
            "inst-a",
            "gpt-6-luna",
        ),
        Some(("inst-b", "claude-haiku-4-5"))
    );

    let merged = merge_path_entries(
        Some("/opt/late:/usr/bin"),
        Some("/usr/bin:/usr/local/bin"),
        false,
    );
    assert_eq!(merged, "/opt/late:/usr/bin:/usr/local/bin");
    assert!(did_host_path_change(Some("/usr/bin"), Some(&merged)));
    assert_eq!(list_login_shell_candidates(Some("/bin/zsh"))[0], "/bin/zsh");
    assert_eq!(
        instances_needing_rebuild_after_path_change(&[("codex", true), ("claudeAgent", false)]),
        vec!["claudeAgent"]
    );

    let win = UsageDateWindow {
        since_time_ms: None,
        until_time_ms: None,
        since_day: "2026-08-02".to_owned(),
        until_day: "2026-08-02".to_owned(),
    };
    let day_of = |ts: i64| {
        if ts < 1_785_628_800_000 {
            "2026-08-01".to_owned()
        } else {
            "2026-08-02".to_owned()
        }
    };
    let f1 = filter_cached_file_for_window(
        vec![WindowedUsageRecordRef {
            dedupe_key: Some("k1".to_owned()),
            timestamp_ms: 1_785_542_400_000,
            payload: 5_u64,
        }],
        &[1_785_542_400_000],
        &win,
        day_of,
    )
    .expect("keeps keyed outside-window record");
    let f2 = filter_cached_file_for_window(
        vec![
            WindowedUsageRecordRef {
                dedupe_key: Some("k1".to_owned()),
                timestamp_ms: 1_785_632_400_000,
                payload: 5_u64,
            },
            WindowedUsageRecordRef {
                dedupe_key: Some("k2".to_owned()),
                timestamp_ms: 1_785_636_000_000,
                payload: 7_u64,
            },
        ],
        &[1_785_636_000_000],
        &win,
        day_of,
    )
    .expect("keeps day-2 records and day-2 malformed");
    let (kept, skipped) = collect_windowed_deduped_usage([f1, f2], &win, day_of);
    assert_eq!(kept, vec![7_u64]);
    assert_eq!(skipped, 1);
    assert!(decode_cached_malformed_diagnostics(2, None).is_some_and(|d| d.requires_full_reparse));
}
