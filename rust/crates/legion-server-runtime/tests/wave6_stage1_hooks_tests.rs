//! Focused Wave 6 Stage 1 provider/SCM hook integration and setup output surrogate boundary tests.

use std::collections::BTreeMap;

use legion_contracts::ids::{ProviderDriverKind, ProviderSessionId, ProviderThreadId};
use legion_contracts::orchestration::{ProviderInteractionMode, Run};
use legion_contracts::policy::RuntimeMode;
use legion_contracts::workspace_folders::WorkspaceScanFoldersInput;
use legion_providers::acp::codec::{JsonRpcId, JsonRpcRequest};
use legion_providers::acp::protocol::METHOD_SESSION_REQUEST_PERMISSION;
use legion_providers::drivers::claude_lifecycle::ClaudeHistoryEntry;
use legion_providers::non_acp::NonAcpTurnUpdate;
use legion_scm::git_ref_cache::IdleLocalStatusTracker;
use legion_server_runtime::acp_protocol_io::{AcpWriter, handle_agent_request_for_driver};
use legion_server_runtime::http_workspace_folders::scan_workspace_folders_service;
use legion_server_runtime::non_acp_subprocess::NonAcpProcessRegistry;
use legion_server_runtime::non_acp_turn_sink::{NonAcpStreamSummary, apply_non_acp_update};
use legion_server_runtime::startup_auto_pull::should_refresh_local_git_status;
use legion_server_runtime::turn_events::TurnEmitContext;
use legion_server_runtime::{
    OUTPUT_LINE_MAX_LENGTH, SetupScriptLineBuffer, apply_claude_resume_env,
};
use serde_json::json;

type Res<T = ()> = Result<T, Box<dyn std::error::Error>>;

#[test]
fn setup_output_surrogate_and_stage1_provider_scm_hooks() -> Res {
    let mut buf = SetupScriptLineBuffer::new(None, Vec::new());
    let lines1 = buf.push_chunk(&format!("x{}\r\n", "🙂".repeat(250)));
    assert_eq!(lines1.len(), 1);
    assert!(lines1[0].encode_utf16().count() <= OUTPUT_LINE_MAX_LENGTH);
    assert_eq!(lines1[0], format!("x{}", "🙂".repeat(199)));
    assert_eq!(
        buf.push_chunk(&format!("{}x\r\n", "🙂".repeat(2_100)))[0],
        "🙂".repeat(200)
    );

    let mut env = BTreeMap::new();
    apply_claude_resume_env(
        &mut env,
        true,
        &[ClaudeHistoryEntry {
            message_type: "assistant".to_owned(),
            model: Some("<synthetic>".to_owned()),
        }],
    );
    assert!(!env.contains_key("CLAUDE_CODE_RESUME_INTERRUPTED_TURN"));
    apply_claude_resume_env(&mut env, true, &[]);
    assert_eq!(
        env.get("CLAUDE_CODE_RESUME_INTERRUPTED_TURN")
            .map(String::as_str),
        Some("1")
    );

    let reg = NonAcpProcessRegistry::new();
    let e1 = reg.schedule_idle_unload("psess-1", "thr-1", "pthr-a")?;
    let _e2 = reg.schedule_idle_unload("psess-1", "thr-1", "pthr-b")?;
    assert_eq!(reg.pending_idle_unloads("psess-1").len(), 2);
    assert!(
        reg.take_idle_unload_if_matches("psess-1", "pthr-a", e1.generation)?
            .is_some()
    );

    let (pipe_r, pipe_w) = std::io::pipe()?;
    let writer = AcpWriter::new(Some(pipe_w));
    let perm_req = JsonRpcRequest {
        jsonrpc: "2.0".to_owned(),
        id: JsonRpcId::Number(7),
        method: METHOD_SESSION_REQUEST_PERMISSION.to_owned(),
        params: Some(json!({
            "sessionId": "s1",
            "toolCall": {
                "toolCallId": "tc1",
                "title": "Running legion-code / legion_workflow",
                "rawInput": { "ServerName": "legion-code", "ToolName": "legion_workflow" }
            },
            "options": [
                { "optionId": "deny", "name": "Deny", "kind": "reject_once" },
                { "optionId": "allow", "name": "Allow", "kind": "allow_once" }
            ]
        })),
    };
    handle_agent_request_for_driver(
        &writer,
        &perm_req,
        RuntimeMode::ApprovalRequired,
        ProviderInteractionMode::Default,
        Some("antigravity"),
    )?;
    drop(writer);
    assert!(std::io::read_to_string(pipe_r)?.contains("\"allow\""));
    Ok(())
}

#[test]
fn stage1_turn_sink_scm_and_folder_ignore_hooks() -> Res {
    let run: Run = serde_json::from_value(json!({
        "id": "run-1", "threadId": "thr-1", "ordinal": 1,
        "providerInstanceId": "claudeAgent",
        "modelSelection": { "instanceId": "claudeAgent", "model": "claude-sonnet-5-1" },
        "providerThreadId": null, "userMessageId": "msg-u1", "rootNodeId": "node-1",
        "activeAttemptId": null, "status": "running", "requestedAt": "2026-11-22T00:00:00.000Z"
    }))?;
    let mut ctx = TurnEmitContext::new(
        &run,
        ProviderDriverKind::new("claudeAgent")?,
        ProviderSessionId::new("ps-1")?,
        ProviderThreadId::new("pt-1")?,
        "2026-11-22T00:00:00.000Z".to_owned(),
        1,
    )?;
    let mut summary = NonAcpStreamSummary::default();
    summary.begin_thinking_block();
    summary.finish_thinking_block(ctx.assistant_message_id.as_str());
    summary.begin_thinking_block();
    let _ = apply_non_acp_update(
        &mut ctx,
        &mut summary,
        NonAcpTurnUpdate::ReasoningDelta {
            text: "Checking repo next.".to_owned(),
        },
        "2026-11-22T00:00:01.000Z",
        "claude-sonnet-5-1",
    )?;
    let tool_ev = apply_non_acp_update(
        &mut ctx,
        &mut summary,
        NonAcpTurnUpdate::ToolCallStarted {
            call_id: "call-1".to_owned(),
            name: "read_file".to_owned(),
            input_json: json!({}),
        },
        "2026-11-22T00:00:02.000Z",
        "claude-sonnet-5-1",
    )?;
    assert!(tool_ev.len() >= 2 && ctx.assistant_text.contains("Checking repo next."));

    let tmp_repo = tempfile::tempdir()?;
    std::fs::create_dir_all(tmp_repo.path().join(".git/refs/heads"))?;
    std::fs::create_dir_all(tmp_repo.path().join(".git/refs/remotes"))?;
    std::fs::write(tmp_repo.path().join(".git/HEAD"), "ref: refs/heads/main\n")?;
    let mut tracker = IdleLocalStatusTracker::new();
    assert!(should_refresh_local_git_status(
        &mut tracker,
        tmp_repo.path(),
        &tmp_repo.path().join(".git"),
        None,
    ));
    assert!(!should_refresh_local_git_status(
        &mut tracker,
        tmp_repo.path(),
        &tmp_repo.path().join(".git"),
        None,
    ));

    let ignored_proj = tempfile::tempdir()?;
    std::fs::create_dir_all(ignored_proj.path().join("api/.git"))?;
    std::fs::write(
        ignored_proj.path().join("t3.json"),
        r#"{"ignoreRepositories":["."]}"#,
    )?;
    let scanned = scan_workspace_folders_service(&WorkspaceScanFoldersInput {
        workspace_root: ignored_proj.path().to_string_lossy().into_owned(),
    })?;
    assert_eq!(scanned.folders.len(), 0);
    Ok(())
}
