//! Wave 4 upstream alignment tests covering Claude, Codex, and Antigravity.

use std::collections::HashSet;
use std::path::Path;

use legion_contracts::orchestration::{ProviderApprovalDecision, ProviderFailureClass};
use legion_contracts::policy::RuntimeMode;
use legion_providers::acp::presentation::ToolCallKind;
use legion_providers::core::{ProviderFailureInput, classify_provider_failure};
use legion_providers::drivers::{
    ANTIGRAVITY_POST_SETTLE_COMMAND_HOLD_MILLIS, AntigravityPermissionDisposition,
    AntigravityTurnPhase, ClaudeHistoryEntry, ClaudeLiveQueryReuse, ClaudeModelSwitchRestartCheck,
    ClaudeThinkingTracker, ClaudeTrackedBackgroundTask, CodexAppServerLifecycleError,
    CodexAppServerRetirementState, CodexIdleThreadUnloadRegistry, PostSettleCommandHoldAction,
    PostSettleCommandHoldState, RememberedAntigravityRuntimeModes,
    antigravity_command_outlives_prompt, antigravity_permission_disposition_for_tool,
    antigravity_tool_outlives_prompt, antigravity_usage_profile_dirs,
    claude_history_ends_on_failed_request, claude_resume_interrupted_turn_env,
    claude_writes_updates_as_thinking, codex_auto_command_approval_decision,
    evaluate_post_settle_command_hold, format_claude_version_upgrade_message,
    is_load_key_for_provider_thread, parse_remembered_antigravity_runtime_modes,
    resolve_claude_lost_background_work, resolve_initial_antigravity_runtime_modes,
    should_persist_remembered_runtime_modes, should_start_updated_claude_on_model_switch,
};
use legion_providers::non_acp::{
    ClaudeStreamUsage, CodexFileUpdateChange, CodexPatchChangeKind, NonAcpTurnUpdate,
    codex_multi_file_patch, parse_claude_stream_json_line,
};
use serde_json::json;

#[test]
fn claude_stream_partial_usage_and_synthetic_frames_do_not_stall() {
    let empty_start = parse_claude_stream_json_line(
        r#"{"type":"stream_event","event":{"type":"message_start","message":{"id":"m1","usage":{}}}}"#,
    )
    .expect("parse empty message_start");
    assert_eq!(empty_start, Vec::new());

    let synthetic = parse_claude_stream_json_line(
        r#"{"type":"assistant","message":{"id":"m2","model":"<synthetic>","usage":{"input_tokens":100,"output_tokens":5}}}"#,
    )
    .expect("parse synthetic");
    assert_eq!(synthetic, Vec::new());

    let start = ClaudeStreamUsage::from_message_start(Some(&json!({"input_tokens": null})));
    assert_eq!(start.total_input_tokens(), 0);
    let partial = start.merge_delta(Some(&json!({"output_tokens": 12.6})));
    assert_eq!(partial.output_tokens, 13);
    let snap = partial.merge_assistant_snapshot(Some(
        &json!({"input_tokens": 50, "cache_read_input_tokens": 100, "output_tokens": 4}),
    ));
    assert_eq!(snap.output_tokens, 13);
    assert_eq!(snap.total_input_tokens(), 150);

    let delta_updates = parse_claude_stream_json_line(
        r#"{"type":"stream_event","event":{"type":"message_delta","usage":{"input_tokens":10,"output_tokens":5}}}"#,
    )
    .expect("parse delta");
    assert_eq!(
        delta_updates,
        vec![NonAcpTurnUpdate::UsageReported {
            input_tokens: 10,
            output_tokens: 5
        }]
    );
}

#[test]
fn claude_lost_work_thinking_resume_and_binary_switch() {
    let buffered: HashSet<String> = ["task-buffered".to_owned()].into_iter().collect();
    let plan = resolve_claude_lost_background_work(
        true,
        true,
        &buffered,
        &[
            ClaudeTrackedBackgroundTask {
                task_id: "task-buffered".to_owned(),
                kind: "shell".to_owned(),
                description: None,
            },
            ClaudeTrackedBackgroundTask {
                task_id: "task-lost".to_owned(),
                kind: "shell".to_owned(),
                description: Some("bg build".to_owned()),
            },
        ],
        &[],
    );
    assert!(plan.should_wake_agent);
    assert_eq!(plan.lost_tasks.len(), 1);
    assert_eq!(plan.lost_tasks[0].task_id, "task-lost");

    assert!(claude_writes_updates_as_thinking("claude-opus-5-5"));
    assert!(claude_writes_updates_as_thinking("claude-sonnet-5-1"));
    assert!(claude_writes_updates_as_thinking(
        "claude-opus-5-1-20261101"
    ));
    assert!(!claude_writes_updates_as_thinking("claude-opus-5-20261101"));
    assert!(!claude_writes_updates_as_thinking("claude-sonnet-4-6"));

    let mut thinking = ClaudeThinkingTracker::new();
    thinking.record_thinking("msg-1", "t-1", "   ");
    thinking.record_thinking("msg-1", "t-2", "I will inspect Cargo.toml next.");
    assert_eq!(
        thinking
            .take_pre_tool_update("claude-opus-5-5", "msg-1")
            .as_deref(),
        Some("I will inspect Cargo.toml next.")
    );

    let failed_history = vec![
        ClaudeHistoryEntry {
            message_type: "user".to_owned(),
            model: None,
        },
        ClaudeHistoryEntry {
            message_type: "assistant".to_owned(),
            model: Some("<synthetic>".to_owned()),
        },
    ];
    assert!(claude_history_ends_on_failed_request(&failed_history));
    assert!(claude_resume_interrupted_turn_env(true, &failed_history).is_none());
    assert!(claude_resume_interrupted_turn_env(true, &failed_history[..1]).is_some());

    assert!(should_start_updated_claude_on_model_switch(
        &ClaudeModelSwitchRestartCheck {
            is_resume: false,
            live_query: ClaudeLiveQueryReuse::MatchingActive,
            started_api_model_id: "claude-sonnet-5-5",
            next_api_model_id: "claude-opus-5-5",
            started_program: Some(Path::new("/opt/claude/2.0.0")),
            current_program: Some(Path::new("/opt/claude/2.1.0")),
            has_running_background_work: false,
        }
    ));

    let cask_msg = format_claude_version_upgrade_message(
        "Claude Opus 5.5",
        "2.1.90",
        Some("2.0.0"),
        Some("2.0.80"),
        Some("brew upgrade --cask claude-code"),
    );
    assert!(cask_msg.contains("brew uninstall --cask claude-code"));
}

#[test]
fn codex_full_access_patch_retirement_and_idle_unload() {
    assert_eq!(
        codex_auto_command_approval_decision(RuntimeMode::FullAccess, false, false, false, 0),
        Some(ProviderApprovalDecision::Accept)
    );
    assert_eq!(
        codex_auto_command_approval_decision(RuntimeMode::FullAccess, false, false, true, 0),
        None
    );

    let patch = codex_multi_file_patch(&[
        CodexFileUpdateChange {
            path: "a.txt".to_owned(),
            kind: CodexPatchChangeKind::Add,
            diff: "hello\n".to_owned(),
        },
        CodexFileUpdateChange {
            path: "b.txt".to_owned(),
            kind: CodexPatchChangeKind::Update {
                move_path: Some("c.txt".to_owned()),
            },
            diff: "--- \"a/b.txt\"\n+++ \"b/c.txt\"\n@@ -1 +1 @@\n-old\n+new\n".to_owned(),
        },
        CodexFileUpdateChange {
            path: "d.txt".to_owned(),
            kind: CodexPatchChangeKind::Delete,
            diff: "bye".to_owned(),
        },
    ]);
    assert!(patch.contains("diff --git \"a/a.txt\" \"b/a.txt\""));
    assert!(patch.contains("--- /dev/null\n+++ \"b/a.txt\""));
    assert!(patch.contains("diff --git \"a/b.txt\" \"b/c.txt\""));
    assert!(patch.contains("\\ No newline at end of file"));

    let mut state = CodexAppServerRetirementState::new();
    let err = state.mark_unresponsive("turn/start");
    assert_eq!(err.tag(), "CodexAppServerUnresponsiveError");
    assert!(err.display_message().contains("within 2 minutes"));
    assert_eq!(
        state.reject_if_retired(),
        Err(CodexAppServerLifecycleError::Retired)
    );
    let classified = classify_provider_failure(&ProviderFailureInput {
        message: "Codex app-server was retired and accepts no new work.".to_owned(),
        ..Default::default()
    });
    assert_eq!(classified.class, ProviderFailureClass::ProviderError);
    assert!(!classified.retryable);

    let mut unload = CodexIdleThreadUnloadRegistry::new();
    let first = unload.schedule_or_bump("sess-1", "app-thread-1", "native-1");
    let second = unload.schedule_or_bump("sess-1", "app-thread-1", "native-2");
    assert_eq!(unload.pending_for_session("sess-1").len(), 2);
    assert!(
        unload
            .take_if_generation_matches("sess-1", "native-1", first.generation)
            .is_some()
    );
    assert_eq!(unload.pending_for_session("sess-1"), vec![second]);
    assert!(is_load_key_for_provider_thread(
        Some(r#"{"providerThread":"codex:c1"}"#),
        "p-1",
        Some("codex"),
        Some("c1"),
    ));
}

#[test]
fn antigravity_mcp_preapprove_modes_hold_and_default_profile_usage() {
    for mode in [RuntimeMode::Auto, RuntimeMode::AutoAcceptEdits] {
        assert_eq!(
            antigravity_permission_disposition_for_tool(
                mode,
                None,
                AntigravityPermissionDisposition::Ask,
                Some("Run mcp_legion-code_legion_thread_list?"),
                None,
            ),
            AntigravityPermissionDisposition::Approve
        );
        assert_eq!(
            antigravity_permission_disposition_for_tool(
                mode,
                None,
                AntigravityPermissionDisposition::Ask,
                Some("MCP Tool delegate_task"),
                Some(&json!({"ServerName": "t3-code", "ToolName": "delegate_task"})),
            ),
            AntigravityPermissionDisposition::Approve
        );
        assert_eq!(
            antigravity_permission_disposition_for_tool(
                mode,
                Some("on-request"),
                AntigravityPermissionDisposition::Ask,
                Some("Run mcp_legion-code_legion_thread_list?"),
                None,
            ),
            AntigravityPermissionDisposition::Ask
        );
        assert_eq!(
            antigravity_permission_disposition_for_tool(
                mode,
                None,
                AntigravityPermissionDisposition::Ask,
                Some("Run mcp_t3-code_preview_status"),
                Some(&json!({"CommandLine": "Run mcp_t3-code_preview_status"})),
            ),
            AntigravityPermissionDisposition::Ask
        );
    }

    let raw_saved =
        r#"{"authMethod":"oauth-personal","modes":["approval-required","auto","full-access"]}"#;
    let parsed = parse_remembered_antigravity_runtime_modes(raw_saved).expect("remembered modes");
    assert_eq!(
        resolve_initial_antigravity_runtime_modes("oauth-personal", Some(&parsed)),
        vec![
            RuntimeMode::ApprovalRequired,
            RuntimeMode::Auto,
            RuntimeMode::FullAccess
        ]
    );
    assert!(!should_persist_remembered_runtime_modes(
        Some(&parsed.modes),
        &RememberedAntigravityRuntimeModes {
            auth_method: "oauth-personal".to_owned(),
            modes: parsed.modes.clone(),
        }
        .modes
    ));

    assert!(antigravity_tool_outlives_prompt(
        &ToolCallKind::Other,
        Some("Run schedule"),
        None
    ));
    assert!(!antigravity_command_outlives_prompt(
        &ToolCallKind::Other,
        Some("Run schedule"),
        None,
    ));
    assert!(antigravity_command_outlives_prompt(
        &ToolCallKind::Execute,
        Some("Run cargo test"),
        Some(&json!({"IsDaemon": false})),
    ));
    assert_eq!(
        evaluate_post_settle_command_hold(&PostSettleCommandHoldState {
            phase: AntigravityTurnPhase::SettledPrompt,
            quiet_since_ms: 0,
            last_progress_at_ms: 0,
            now_ms: ANTIGRAVITY_POST_SETTLE_COMMAND_HOLD_MILLIS,
            hold_timeout_ms: ANTIGRAVITY_POST_SETTLE_COMMAND_HOLD_MILLIS,
            has_running_command: true,
            has_other_deferred_work: false,
        }),
        PostSettleCommandHoldAction::FinalizeTurn
    );

    let dirs = antigravity_usage_profile_dirs(Path::new("/state"), ["antigravity"]);
    assert_eq!(dirs.len(), 1);
    assert!(
        dirs[0]
            .to_string_lossy()
            .replace('\\', "/")
            .contains(
                "ac0a3dfd6dddb20962cecff6ee5fe65e19d3923be20e52c5ab52ff877f7e4c32/antigravity-acp/conversations"
            )
    );
}
