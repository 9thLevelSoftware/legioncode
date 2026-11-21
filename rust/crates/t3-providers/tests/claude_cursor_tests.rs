//! Integration tests for Wave 3a Claude, Codex parser, and Cursor fixes.

use std::path::{Path, PathBuf};

use t3_providers::drivers::{
    CLAUDE_ACTIVE_STEERING_INTERRUPTS_TOOLS, CURSOR_SUPPORTS_BACKGROUND_WAIT, ClaudeAssistantPhase,
    ClaudeTextGenRetryDecision, ClaudeTextOnlyModelsCache, CursorRuntimeInstructionTracker,
    CursorSdkErrorKind, CursorSdkModel, CursorSdkParam, CursorSdkVariant,
    CursorTrackedBackgroundShell, CursorWorkerMessage, CursorWorkerRequest,
    MAX_WORKER_ERROR_CAUSE_DEPTH, SerializedBackgroundShellEvent, SerializedWorkerError,
    await_cursor_background_shells, check_text_generation_provider_usable,
    classify_claude_text_gen_retry, claude_fable_supports_fast_mode,
    claude_gateway_cooldown_reset_at, claude_launch_arg_plugin_directories,
    cursor_background_shell_detail, cursor_sdk_model_selection, decode_cursor_worker_request,
    encode_cursor_worker_message, is_claude_images_unsupported_error, phase_for_held_steer_result,
    resolve_claude_inline_plugin_directories, summarize_claude_tool_request,
    truncate_without_trailing_high_surrogate,
};

#[test]
fn claude_text_gen_fallback_cooldown_plugins_and_truncation() {
    // 7fcec368cb + 6f82dd9e79: image fallback + memoized text-only models cache
    const {
        assert!(!CLAUDE_ACTIVE_STEERING_INTERRUPTS_TOOLS);
    }
    assert!(is_claude_images_unsupported_error(
        "",
        "Provider error: model does not support image inputs"
    ));
    assert_eq!(
        classify_claude_text_gen_retry(
            1,
            "error: unknown option '--no-session-persistence'",
            true,
            true
        ),
        ClaudeTextGenRetryDecision::RetryWithoutNoSessionPersistence
    );
    assert_eq!(
        classify_claude_text_gen_retry(1, "image unsupported", true, false),
        ClaudeTextGenRetryDecision::RetryWithoutImages
    );
    let cache = ClaudeTextOnlyModelsCache::new();
    assert!(!cache.is_text_only("custom-text-model"));
    assert!(cache.record_if_unsupported_then_succeeded(
        "custom-text-model",
        "",
        "Images are unsupported for this endpoint",
        true,
    ));
    assert!(cache.is_text_only("custom-text-model"));

    // e3294d7663: untyped unknown rejection drops null placeholder when gateway reset_at present
    assert_eq!(
        claude_gateway_cooldown_reset_at(
            &[(Some("unknown"), None), (Some("five_hour"), Some(2_000))],
            None,
            Some(5_000),
        ),
        Some(5_000)
    );

    // 0b02be3d5f: pre-late-read-steer assistant phase
    assert_eq!(
        phase_for_held_steer_result(true, true),
        Some(ClaudeAssistantPhase::FinalAnswer)
    );

    // a1b9e1cc87: repeatable --plugin-dir + valueFlags
    let args = "--append-system-prompt --plugin-dir --plugin-dir ./p1 --plugin-dir=/abs/p2 -- --plugin-dir ignored";
    assert_eq!(
        claude_launch_arg_plugin_directories(Some(args)),
        vec!["./p1", "/abs/p2"]
    );
    let cwd_root = if cfg!(windows) {
        r"C:\workspace"
    } else {
        "/workspace"
    };
    let home_root = if cfg!(windows) {
        r"C:\Users\alice"
    } else {
        "/home/alice"
    };
    let dirs = resolve_claude_inline_plugin_directories(
        &["rel".to_owned(), "~/not-expanded".to_owned()],
        Some("relative-ignored:~/env-plugin"),
        Some(Path::new(cwd_root)),
        Path::new(home_root),
        ':',
    );
    assert_eq!(
        dirs,
        vec![
            PathBuf::from(cwd_root).join("rel"),
            PathBuf::from(cwd_root).join("~/not-expanded"),
            PathBuf::from(home_root).join("env-plugin"),
        ]
    );

    // 76bd3bb098 + c688453acd + 6f5293fbdc
    assert!(claude_fable_supports_fast_mode("fable-5.2"));
    assert!(!claude_fable_supports_fast_mode("opus-4"));
    let emoji = format!("{}🚀end", "a".repeat(399));
    let safe = truncate_without_trailing_high_surrogate(&emoji, 400);
    assert_eq!(safe, "a".repeat(399));
    assert!(summarize_claude_tool_request("Bash", Some(&emoji), None, "{}").len() <= 406);
    assert!(check_text_generation_provider_usable(Some(false), true, true).is_err());
}

#[test]
fn cursor_worker_framing_background_shells_max_mode_and_instructions() {
    const {
        assert!(CURSOR_SUPPORTS_BACKGROUND_WAIT);
    }

    // 29af131f06: cause depth clamping + NDJSON roundtrip
    let mut deepest = SerializedWorkerError {
        name: "Error".to_owned(),
        message: "root".to_owned(),
        stack: None,
        code: None,
        status: None,
        sdk_error_kind: Some(CursorSdkErrorKind::RateLimit),
        cause: None,
    };
    for idx in 0..=MAX_WORKER_ERROR_CAUSE_DEPTH + 2 {
        deepest = SerializedWorkerError {
            name: "Error".to_owned(),
            message: format!("lvl-{idx}"),
            stack: None,
            code: None,
            status: None,
            sdk_error_kind: None,
            cause: Some(Box::new(deepest)),
        };
    }
    let clamped = deepest.clamp_cause_depth();
    let mut cur = &clamped;
    let mut hops = 0;
    while let Some(inner) = cur.cause.as_deref() {
        hops += 1;
        cur = inner;
    }
    assert_eq!(hops, MAX_WORKER_ERROR_CAUSE_DEPTH);

    let frame = CursorWorkerMessage::BackgroundShell {
        session_id: 7,
        event: SerializedBackgroundShellEvent::Started {
            id: "sh-1".to_owned(),
            pid: Some(4242),
            command: "cargo test".to_owned(),
        },
    };
    let encoded = encode_cursor_worker_message(&frame).expect("encode");
    assert!(encoded.ends_with('\n'));
    let req = decode_cursor_worker_request(r#"{"type":"close","id":1}"#).expect("decode close");
    assert!(matches!(req, CursorWorkerRequest::Close { id: 1 }));

    // b5453de6b5: background wait by id or pid + detail truncation
    let mut shells = vec![CursorTrackedBackgroundShell {
        id: "sh-1".to_owned(),
        pid: Some(4242),
        command: "npm test".to_owned(),
        awaited: false,
    }];
    let (matched, unmatched) = await_cursor_background_shells(&mut shells, &["4242", "missing"]);
    assert_eq!(matched, vec!["4242"]);
    assert_eq!(unmatched, vec!["missing"]);
    assert!(shells[0].awaited);
    let detail = cursor_background_shell_detail(&"c".repeat(220), Some(1), Some("/tmp/out.log"));
    assert!(detail.contains("Background command completed (exit 1):"));
    assert!(detail.contains("..."));
    assert!(detail.contains("Output: /tmp/out.log"));

    // 57e92b0b98: Max Mode tier preserved across model switches
    let sdk_models = vec![CursorSdkModel {
        id: "composer-2".to_owned(),
        aliases: vec!["composer-alias".to_owned()],
        variants: vec![CursorSdkVariant {
            params: vec![CursorSdkParam {
                id: "thinking".to_owned(),
                value: "high".to_owned(),
            }],
            is_max_mode: true,
        }],
    }];
    let selected = cursor_sdk_model_selection(
        "composer-alias",
        vec![CursorSdkParam {
            id: "thinking".to_owned(),
            value: "high".to_owned(),
        }],
        &sdk_models,
    );
    assert!(selected.max_mode);
    assert_eq!(
        cursor_sdk_model_selection("auto", vec![], &sdk_models).id,
        "default"
    );

    // ed203c46cb: runtime instructions delivered once per session, reset on compaction/rewind
    let mut tracker = CursorRuntimeInstructionTracker::new();
    assert!(tracker.should_send("ag-1", "instr-v1"));
    tracker.record_run_outcome("ag-1", Some("instr-v1"), true, false);
    assert!(!tracker.should_send("ag-1", "instr-v1"));
    assert!(tracker.should_send("ag-1", "instr-v2"));
    tracker.record_run_outcome("ag-1", Some("instr-v1"), true, true);
    assert!(tracker.should_send("ag-1", "instr-v1"));
}
