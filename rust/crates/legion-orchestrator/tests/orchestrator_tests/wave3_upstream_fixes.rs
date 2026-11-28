use std::sync::{Arc, Mutex};

use legion_contracts::ids::{CommandId, NodeId, ProjectId, RunId, ThreadId};
use legion_contracts::orchestration::{
    CreationSource, DelegatedCompletionRef, DispatchMode, OrchestrationActor, OrchestrationCommand,
    OrchestrationNotification, ProviderInteractionMode, RunStatus, ThreadCreateCommand,
};
use legion_contracts::policy::RuntimeMode;
use legion_orchestrator::Orchestrator;
use legion_orchestrator::decider::queue_ops::{
    compute_follows_thread_selection, ensure_steer_allows_command, format_provider_turn_prompt,
    is_slash_command_message, plan_missed_steer_requeue, resolve_steer_fallback_model_selection,
    should_defer_context_for_turn,
};
use legion_orchestrator::decider::title_ops::{
    decide_thread_create_with_title_renamed, effective_message_created_by,
    effective_message_created_by_json, should_seed_title_from_first_message,
};
use legion_orchestrator::projections::query_wire_thread_detail;
use legion_orchestrator::sink::projection_apply::DeciderThreadState;
use legion_persistence::sqlite::SqliteStore;
use serde_json::json;

use super::helpers::TestResult;
use super::wave3_helpers::{basic_model, make_dispatch_cmd, make_run, make_user_message};

#[test]
fn slash_commands_pass_verbatim_reject_non_restart_steer_and_defer_context() -> TestResult {
    let mut msg = make_user_message("m-1", "t-1", "/review --strict", None)?;
    assert!(is_slash_command_message(&msg));
    assert!(should_defer_context_for_turn(&msg));
    assert_eq!(
        format_provider_turn_prompt(&msg, "PRIOR CONTEXT", "/review --strict"),
        "/review --strict"
    );

    msg.text = "/home/user/repo.rs check this".to_owned();
    assert!(!is_slash_command_message(&msg));
    assert_eq!(
        format_provider_turn_prompt(&msg, "PRIOR CONTEXT", &msg.text),
        "PRIOR CONTEXT\n\nUser message:\n/home/user/repo.rs check this"
    );

    assert!(ensure_steer_allows_command(false, "/compact", &[]).is_err());
    assert!(ensure_steer_allows_command(true, "/compact", &[]).is_ok());
    Ok(())
}

#[test]
fn missed_steer_keeps_queue_order_releases_held_and_excludes_non_user_follows() -> TestResult {
    let model = basic_model()?;
    let cmd_base = make_dispatch_cmd("cmd-q", "t-1", "m-q", "Follow up")?;
    assert_eq!(
        compute_follows_thread_selection(&cmd_base, true),
        Some(true)
    );

    let mut delegated = cmd_base.clone();
    delegated.delegated_completion = Some(DelegatedCompletionRef {
        parent_run_id: RunId::new("r-parent")?,
        generation: 1,
        task_ids: vec![NodeId::new("task-1")?],
    });
    assert_eq!(compute_follows_thread_selection(&delegated, true), None);

    let mut compact_wake = cmd_base.clone();
    compact_wake.idle_compaction = Some("idle".to_owned());
    assert_eq!(compute_follows_thread_selection(&compact_wake, true), None);

    let mut notify_cmd = cmd_base;
    notify_cmd.notification = Some(OrchestrationNotification {
        source: json!({ "kind": "monitor" }),
        outcome: "updated".to_owned(),
        summary: "ok".to_owned(),
        detail: None,
    });
    assert_eq!(compute_follows_thread_selection(&notify_cmd, true), None);

    let mut msg_follows = make_user_message("m-steer", "t-1", "Steer text", None)?;
    msg_follows.follows_thread_selection = Some(true);
    assert_eq!(
        resolve_steer_fallback_model_selection(Some(&msg_follows), model),
        None
    );

    let r1 = make_run("r-1", "t-1", 1, RunStatus::Completed)?;
    let mut r2_later = make_run("r-2", "t-1", 3, RunStatus::Queued)?;
    r2_later.queue_position = Some(Some(1));
    r2_later.queue_held = Some(true);
    let mut r3_even_later = make_run("r-3", "t-1", 4, RunStatus::Queued)?;
    r3_even_later.queue_position = Some(Some(2));
    let placement = plan_missed_steer_requeue(&[r1, r2_later, r3_even_later], 1);
    assert_eq!(placement.queue_position, 1);
    assert_eq!(placement.shifted_runs.len(), 2);
    assert_eq!(placement.shifted_runs[0].queue_position, Some(Some(2)));
    assert_eq!(placement.shifted_runs[0].queue_held, Some(false));
    assert_eq!(placement.shifted_runs[1].queue_position, Some(Some(3)));
    Ok(())
}

#[tokio::test]
async fn idle_compaction_excluded_from_user_attribution_and_custom_title_kept() -> TestResult {
    let t0 = "2026-11-22T10:00:00.000Z";
    let t1 = "2026-11-22T10:01:00.000Z";
    let state = DeciderThreadState::default();
    let create_cmd = ThreadCreateCommand {
        created_by: OrchestrationActor::User,
        creation_source: CreationSource::Mcp,
        command_id: CommandId::new("cmd-create")?,
        thread_id: ThreadId::new("thr-title")?,
        project_id: ProjectId::new("proj-1")?,
        title: "Named before message".to_owned(),
        generate_title: None,
        model_selection: basic_model()?,
        runtime_mode: RuntimeMode::FullAccess,
        interaction_mode: ProviderInteractionMode::Default,
        branch: None,
        worktree_path: None,
        link_origin: None,
        delegated_from: None,
        imported_native_thread: None,
        initial_message: None,
        title_renamed: None,
        workflow_run: None,
    };
    let decision = decide_thread_create_with_title_renamed(&state, &create_cmd, true, t0)?;
    let thread = decision.resulting_thread.ok_or("missing created thread")?;
    assert_eq!(thread.title_renamed, Some(true));
    assert!(!should_seed_title_from_first_message(
        &thread,
        Some("Overwriting seed"),
        false,
        OrchestrationActor::User,
        true
    ));

    let db = Arc::new(Mutex::new(SqliteStore::open_in_memory()?.into_connection()));
    let orch = Orchestrator::new(Arc::clone(&db));
    orch.dispatch_command(&OrchestrationCommand::ThreadCreate(create_cmd), t0)
        .await?;
    let mut compact_msg_cmd =
        make_dispatch_cmd("cmd-compact", "thr-title", "m-compact", "/compact")?;
    compact_msg_cmd.creation_source = CreationSource::Server;
    compact_msg_cmd.dispatch_mode = DispatchMode::StartImmediately;
    compact_msg_cmd.idle_compaction = Some("idle".to_owned());
    orch.dispatch_command(&OrchestrationCommand::MessageDispatch(compact_msg_cmd), t1)
        .await?;
    let detail = {
        let conn = db.lock().map_err(|e| e.to_string())?;
        query_wire_thread_detail(&conn, "thr-title")?
    };
    let msg_json = detail
        .projection
        .messages
        .first()
        .ok_or("missing compact msg")?;
    assert_eq!(msg_json["createdBy"], "system");
    assert_eq!(effective_message_created_by_json(msg_json), "system");

    let mut typed_msg = make_user_message("m-2", "thr-title", "/compact", None)?;
    typed_msg.idle_compaction = Some("idle".to_owned());
    assert_eq!(
        effective_message_created_by(&typed_msg),
        OrchestrationActor::System
    );
    Ok(())
}
