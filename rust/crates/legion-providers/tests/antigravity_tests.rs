//! Integration tests covering Wave 3a Antigravity lifecycle, modes, ACP tool call
//! presentation, attachments, and usage pricing fixes.

use legion_contracts::policy::RuntimeMode;
use legion_providers::acp::{
    AcpToolCallState, ToolActivityCategory, ToolCallKind, ToolCallStatus, ToolCallUpdatePayload,
    commandless_command_label, extract_mcp_tool_call_identity, map_tool_call_presentation,
};
use legion_providers::core::{
    ServerModelEntry, TextAttachmentDecodeError, decode_text_attachment_bytes,
};
use legion_providers::drivers::{
    AntigravityModelCapability, AntigravityPermissionDisposition, AntigravityThinkingDescriptor,
    InactivitySliceAction, ProviderCatalogModels, RawAcpUsageCounts, acp_per_turn_prompt_usage,
    antigravity_model_id_lookup, antigravity_permission_disposition, antigravity_permission_mode,
    antigravity_rate_model, antigravity_runtime_mode_for_session_mode, antigravity_session_mode,
    antigravity_tool_outlives_prompt, bound_antigravity_command_text, can_steer_antigravity_turn,
    catalog_model_label, classify_antigravity_turn_failure, evaluate_turn_inactivity_slice,
    has_unread_waking_inbox_message, is_antigravity_background_wait_tool,
    is_antigravity_conversation_db_tables, is_antigravity_subagent_killed,
    read_complete_jsonl_tail, resolve_antigravity_suffixed_model,
    sanitize_antigravity_command_fields, select_antigravity_text_gen_thinking,
    should_finalize_worktree_handoff_turn,
};
use serde_json::{Map, json};
use tempfile::tempdir;

#[test]
fn antigravity_turn_lifecycle_and_modes() {
    // 990f31cac2: host sleep overrun resets activity window
    assert_eq!(
        evaluate_turn_inactivity_slice(0, 120_000, Some(10_000), 60_000, 100_000),
        InactivitySliceAction::ResetProgressAfterHostSleep {
            new_last_progress_at: 100_000,
        }
    );

    // 505f6d6cf4 + 6edb53026b + 9f6f5540cf + e24f0c7154
    assert!(is_antigravity_background_wait_tool(
        "legion_background_wait"
    ));
    assert!(antigravity_tool_outlives_prompt(
        &ToolCallKind::Other,
        Some("Running schedule"),
        None
    ));
    assert!(antigravity_tool_outlives_prompt(
        &ToolCallKind::Execute,
        None,
        None
    ));
    assert!(!antigravity_tool_outlives_prompt(
        &ToolCallKind::Execute,
        None,
        Some(&json!({ "IsDaemon": true }))
    ));
    assert!(should_finalize_worktree_handoff_turn(true, true, false));
    assert!(can_steer_antigravity_turn(true, true, 0));
    assert!(is_antigravity_subagent_killed(Some(
        "SUBAGENT_STATE_KILLED"
    )));

    // 5ee1c2f652 + e74b795535 + 5dd565d6b1
    let models = vec![AntigravityModelCapability {
        entry: ServerModelEntry {
            slug: "gemini-3.8-flash".to_owned(),
            name: "Gemini 3.8 Flash".to_owned(),
            short_name: "3.8 Flash".to_owned(),
            is_custom: false,
            supports_reasoning: true,
        },
        aliases: vec![],
        thinking: Some(AntigravityThinkingDescriptor {
            option_id: "thinking_level".to_owned(),
            allowed_values: vec!["low".to_owned(), "high".to_owned()],
        }),
    }];
    let suffixed = resolve_antigravity_suffixed_model(&models, "gemini-3.8-flash-high")
        .expect("resolved thinking suffix");
    assert_eq!(suffixed.model, "gemini-3.8-flash");
    assert_eq!(suffixed.value, "high");
    assert_eq!(
        select_antigravity_text_gen_thinking(
            &[("thinking_level".to_owned(), "high".to_owned())],
            "thinking_level",
            "low",
            &["low".to_owned(), "high".to_owned()],
        ),
        Some("high".to_owned())
    );
    assert_eq!(antigravity_permission_mode(RuntimeMode::Auto), "auto");
    assert_eq!(
        antigravity_session_mode("auto", &["default", "auto"]),
        "auto"
    );
    assert_eq!(
        antigravity_session_mode("yolo", &["default", "auto"]),
        "default"
    );
    assert_eq!(
        antigravity_runtime_mode_for_session_mode("auto"),
        Some(RuntimeMode::Auto)
    );
    assert_eq!(
        antigravity_permission_disposition(
            RuntimeMode::Auto,
            AntigravityPermissionDisposition::Approve
        ),
        AntigravityPermissionDisposition::Ask
    );
    assert_eq!(
        antigravity_permission_disposition(
            RuntimeMode::Auto,
            AntigravityPermissionDisposition::Deny
        ),
        AntigravityPermissionDisposition::Deny
    );
}

#[test]
fn antigravity_attachments_mcp_failures_and_usage() {
    // f68d36d8aa + df09ced768 + ff8a831067 + 7f928416b2 + 9294a9801e
    let failure = classify_antigravity_turn_failure(
        "NOT_FOUND (code 404): Requested entity was not found.",
        true,
    )
    .expect("classified 404");
    assert!(failure.retryable);
    assert_eq!(failure.resume_with, Some("continue"));
    let pre_model =
        classify_antigravity_turn_failure("NOT_FOUND (code 404): Unknown model.", false)
            .expect("classified pre-model 404");
    assert!(!pre_model.retryable);

    let cmd = format!("prefix-🚀{}", "x".repeat(20));
    let bounded = bound_antigravity_command_text(&cmd, 20);
    assert!(bounded.starts_with("... [truncated]\n"));
    let mut obj = Map::new();
    obj.insert(
        "CommandLine".to_owned(),
        json!(format!("git-status-head-{}", "a".repeat(9000))),
    );
    obj.insert(
        "output".to_owned(),
        json!(format!("{}tail-end", "z".repeat(9000))),
    );
    let sanitized = sanitize_antigravity_command_fields(&obj);
    assert!(
        sanitized["CommandLine"]
            .as_str()
            .expect("str")
            .starts_with("git-status-head-")
    );
    assert!(
        sanitized["CommandLine"]
            .as_str()
            .expect("str")
            .ends_with("\n\n[Command truncated]")
    );
    assert!(
        sanitized["output"]
            .as_str()
            .expect("str")
            .starts_with("... [truncated]\n")
    );
    assert!(
        sanitized["output"]
            .as_str()
            .expect("str")
            .ends_with("tail-end")
    );

    let utf16_le = [0xFF, 0xFE, b'h', 0x00, b'i', 0x00];
    assert_eq!(
        decode_text_attachment_bytes("a.txt", &utf16_le).expect("utf16"),
        "hi"
    );
    assert!(matches!(
        decode_text_attachment_bytes("bad.txt", &[0xFF, 0xFE, b'a']),
        Err(TextAttachmentDecodeError::InvalidEncoding {
            encoding: "UTF-16LE",
            ..
        })
    ));

    let mcp = extract_mcp_tool_call_identity(
        "MCP Tool navigate_page",
        Some(&json!({"ServerName": "chrome-devtools", "ToolName": "navigate_page", "Arguments": {"url": "https://example.com"}})),
        None,
    )
    .expect("mcp identity");
    assert_eq!(mcp.identity.server, "chrome-devtools");
    assert_eq!(mcp.identity.tool, "navigate_page");
    let view = map_tool_call_presentation(&AcpToolCallState::from_payload(ToolCallUpdatePayload {
        tool_call_id: "tc-mcp".to_owned(),
        title: Some("MCP Tool navigate_page".to_owned()),
        kind: Some(ToolCallKind::Execute),
        status: Some(ToolCallStatus::Completed),
        locations: None,
        content: None,
        raw_input: Some(json!({"ServerName": "chrome-devtools", "ToolName": "navigate_page"})),
        raw_output: None,
    }));
    assert_eq!(view.category, ToolActivityCategory::McpTool);
    assert_eq!(
        commandless_command_label(
            "command_execution",
            None,
            Some("Ran command"),
            &ToolCallStatus::InProgress
        ),
        Some("Running command")
    );
}

#[test]
fn antigravity_usage_pricing_inbox_and_jsonl_tail() {
    // 602e5b32f5 + 755de001f2 + 46b626263f + 490a0d687b
    let usage = acp_per_turn_prompt_usage(
        true,
        RawAcpUsageCounts {
            input_tokens: 100,
            output_tokens: 50,
            cached_read_tokens: 40,
            cached_write_tokens: 10,
            thought_tokens: 19,
        },
    );
    assert_eq!(usage.input_tokens, 140);
    assert_eq!(usage.output_tokens, 50);
    assert_eq!(usage.reasoning_tokens, 19);
    assert_eq!(
        antigravity_model_id_lookup(1537),
        Some("gemini-pro-orchestrator-neon")
    );
    assert_eq!(
        antigravity_rate_model("gemini-pro-orchestrator-neon"),
        "gemini-4-neon"
    );
    assert!(!is_antigravity_conversation_db_tables([
        "sqlite_sequence",
        "summaries"
    ]));
    assert!(is_antigravity_conversation_db_tables([
        "steps",
        "gen_metadata"
    ]));
    let providers = [ProviderCatalogModels {
        driver: "antigravity",
        models: &[(
            ServerModelEntry {
                slug: "gemini-4-neon".to_owned(),
                name: "Gemini 4 Neon".to_owned(),
                short_name: "Gemini 4 Neon".to_owned(),
                is_custom: false,
                supports_reasoning: true,
            },
            &["gemini-pro-orchestrator-neon"],
        )],
    }];
    assert_eq!(
        catalog_model_label(&providers, &["gemini-pro-orchestrator-neon"]).as_deref(),
        Some("gemini-4-neon")
    );

    // 3a91757430 + ad4d0f0659
    let dir = tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("m1.json"),
        r#"{"id":"m1","priority":"MESSAGE_PRIORITY_HIGH"}"#,
    )
    .expect("write");
    assert!(has_unread_waking_inbox_message(dir.path()));
    std::fs::write(dir.path().join("read.json"), r#"{"m1":true}"#).expect("write");
    assert!(!has_unread_waking_inbox_message(dir.path()));

    let jl = dir.path().join("transcript.jsonl");
    std::fs::write(&jl, "{\"step\":1}\n{\"step\":2}\n").expect("write");
    let clipped = read_complete_jsonl_tail(&jl, 10).expect("clipped tail");
    assert_eq!(clipped.lines, Vec::<String>::new());
    assert!(clipped.truncated);
    let exact = read_complete_jsonl_tail(&jl, 11).expect("exact boundary tail");
    assert_eq!(exact.lines, vec!["{\"step\":2}"]);
    assert!(exact.truncated);
}
