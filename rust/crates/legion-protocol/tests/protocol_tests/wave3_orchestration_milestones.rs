use std::error::Error;

use legion_contracts::ids::{
    EventId, MessageId, NodeId, ProjectId, ProviderInstanceId, RuntimeRequestId, ThreadId,
};
use legion_contracts::orchestration::{
    AppThread, ConversationMessage, CreationSource, DomainEvent, DomainEventPayload,
    ModelSelection, OrchestrationActor, OrchestrationNotification, PatchValue,
    ProviderInteractionMode, RuntimeRequest, ThreadLineage,
};
use legion_contracts::policy::RuntimeMode;
use legion_protocol::milestones::{
    MilestoneEvent, MilestoneGuestResolution, MilestoneHub, MilestoneInputKind,
    MilestoneInputResolution, MilestoneRecentOptions, OrchestrationMilestones,
    WorkflowMilestoneMeta, parse_iso_epoch_ms,
};
use serde_json::json;

type TestResult = Result<(), Box<dyn Error>>;

fn sample_thread(id: &str, is_subagent: bool) -> Result<AppThread, Box<dyn Error>> {
    let tid = ThreadId::new(id)?;
    let pid = ProjectId::new("proj-1")?;
    let inst = ProviderInstanceId::new("codex")?;
    Ok(AppThread {
        created_by: OrchestrationActor::User,
        creation_source: CreationSource::Web,
        id: tid,
        project_id: pid,
        title: "Thread".to_owned(),
        provider_instance_id: inst.clone(),
        model_selection: ModelSelection {
            instance_id: inst,
            model: "gpt-5".to_owned(),
            options: None,
        },
        runtime_mode: RuntimeMode::FullAccess,
        interaction_mode: ProviderInteractionMode::Default,
        branch: None,
        worktree_path: None,
        linked_pull_request: None,
        pull_requests: None,
        branch_pull_request: None,
        issues: None,
        active_provider_thread_id: None,
        history_origin: None,
        link_origin: None,
        delegated_from: None,
        lineage: ThreadLineage {
            parent_thread_id: is_subagent.then(|| ThreadId::new("parent-1")).transpose()?,
            relationship_to_parent: is_subagent.then(|| "subagent".to_owned()),
            root_thread_id: ThreadId::new("parent-1")?,
            side_chat: None,
            workflow_step: None,
        },
        forked_from: None,
        created_at: "2026-11-22T00:00:00.000Z".to_owned(),
        updated_at: "2026-11-22T00:00:00.000Z".to_owned(),
        archived_at: None,
        settled_override: None,
        settled_at: None,
        unsettled_at: None,
        snoozed_until: None,
        snoozed_at: None,
        last_snooze_wake_at: None,
        limit_recovery: None,
        guest_share: None,
        pinned_at: None,
        auto_settle_disabled_at: None,
        pin_order_key: None,
        active_order_key: None,
        tags: None,
        last_visited_at: None,
        opens: PatchValue::Absent,
        title_regeneration: None,
        last_title_regeneration: None,
        title_renamed: None,
        rollback_request_id: None,
        rollback_failure: None,
        rollback_unrestored: None,
        workflow_run: None,
        deleted_at: None,
    })
}

fn make_req_event(
    thread_id: &ThreadId,
    status: &str,
    resolved_at: Option<&str>,
) -> Result<DomainEvent, Box<dyn Error>> {
    Ok(DomainEvent {
        id: EventId::new(format!("ev-req-{status}"))?,
        thread_id: thread_id.clone(),
        run_id: None,
        node_id: Some(NodeId::new("node-1")?),
        driver: None,
        provider_instance_id: None,
        raw_event_id: None,
        occurred_at: "2026-11-22T10:01:00.000Z".to_owned(),
        payload: DomainEventPayload::RuntimeRequestUpdated(RuntimeRequest {
            id: RuntimeRequestId::new("req-1")?,
            node_id: NodeId::new("node-1")?,
            provider_turn_id: None,
            native_request_ref: None,
            kind: "user_input".to_owned(),
            status: status.to_owned(),
            response_capability: json!({ "type": "live" }),
            created_at: "2026-11-22T10:00:30.000Z".to_owned(),
            resolved_at: resolved_at.map(str::to_owned),
            decision: None,
            feedback: None,
            answers: None,
        }),
    })
}

fn make_guest_event(
    id: &str,
    thread: &AppThread,
    occurred_at: &str,
) -> Result<DomainEvent, Box<dyn Error>> {
    Ok(DomainEvent {
        id: EventId::new(id)?,
        thread_id: thread.id.clone(),
        run_id: None,
        node_id: None,
        driver: None,
        provider_instance_id: None,
        raw_event_id: None,
        occurred_at: occurred_at.to_owned(),
        payload: DomainEventPayload::ThreadState {
            event_type: "thread.guest-share-updated".to_owned(),
            thread: Box::new(thread.clone()),
        },
    })
}

#[test]
fn orchestration_milestones_project_requests_and_guest_closure() -> TestResult {
    let since_ms = parse_iso_epoch_ms("2026-11-22T10:00:00.000Z").ok_or("bad ts")?;
    let mut proj = OrchestrationMilestones::new(since_ms);
    let hub = MilestoneHub::new();
    let mut thread = sample_thread("thr-1", false)?;
    let wf_meta = WorkflowMilestoneMeta {
        workflow_run_id: Some("wf-42".to_owned()),
    };

    proj.handle_and_publish(
        &hub,
        &make_req_event(&thread.id, "pending", None)?,
        Some(&thread),
        Some(&wf_meta),
    );
    proj.handle_and_publish(
        &hub,
        &make_req_event(&thread.id, "resolved", Some("2026-11-22T10:00:45.000Z"))?,
        Some(&thread),
        Some(&wf_meta),
    );

    thread.guest_share = Some(Some(json!({
        "id": "share-1",
        "guestName": "Alex",
        "messages": [
            { "id": "gm-1", "status": "pending", "submittedAt": "2026-11-22T10:01:00.000Z" }
        ]
    })));
    proj.handle_and_publish(
        &hub,
        &make_guest_event("ev-guest-1", &thread, "2026-11-22T10:01:00.000Z")?,
        Some(&thread),
        Some(&wf_meta),
    );
    thread.guest_share = Some(None);
    proj.handle_and_publish(
        &hub,
        &make_guest_event("ev-guest-2", &thread, "2026-11-22T10:02:00.000Z")?,
        Some(&thread),
        Some(&wf_meta),
    );

    let items = hub.recent(&MilestoneRecentOptions::default()).events;
    assert_eq!(items.len(), 4);
    assert!(matches!(
        &items[0],
        MilestoneEvent::ThreadNeedsInput {
            kind: MilestoneInputKind::UserInput,
            ..
        }
    ));
    assert!(matches!(
        &items[1],
        MilestoneEvent::ThreadInputResolved {
            resolution: MilestoneInputResolution::Resolved,
            ..
        }
    ));
    assert!(matches!(
        &items[2],
        MilestoneEvent::ThreadGuestRequest { .. }
    ));
    assert!(matches!(
        &items[3],
        MilestoneEvent::ThreadGuestRequestResolved {
            resolution: MilestoneGuestResolution::Closed,
            ..
        }
    ));
    Ok(())
}

#[test]
fn orchestration_milestones_project_pr_watch_wake_with_workflow_meta() -> TestResult {
    let since_ms = parse_iso_epoch_ms("2026-11-22T10:00:00.000Z").ok_or("bad ts")?;
    let mut proj = OrchestrationMilestones::new(since_ms);
    let hub = MilestoneHub::new();
    let thread = sample_thread("thr-1", false)?;
    let wf_meta = WorkflowMilestoneMeta {
        workflow_run_id: Some("wf-42".to_owned()),
    };

    let wake_ev = DomainEvent {
        id: EventId::new("ev-pr-wake")?,
        thread_id: thread.id.clone(),
        run_id: None,
        node_id: None,
        driver: None,
        provider_instance_id: None,
        raw_event_id: None,
        occurred_at: "2026-11-22T10:03:00.000Z".to_owned(),
        payload: DomainEventPayload::MessageUpdated(Box::new(ConversationMessage {
            notification: Some(OrchestrationNotification {
                source: json!({
                    "kind": "monitor",
                    "pullRequest": { "host": "github.com", "repository": "o/r", "number": 9, "watchEnded": true }
                }),
                outcome: "completed".to_owned(),
                summary: "PR #9 merged".to_owned(),
                detail: None,
            }),
            created_by: OrchestrationActor::System,
            creation_source: CreationSource::Server,
            scheduled_task_id: None,
            follows_thread_selection: None,
            sender_thread_id: None,
            linked_sender: None,
            idle_compaction: None,
            workflow_script: None,
            id: MessageId::new("msg-pr-1")?,
            thread_id: thread.id.clone(),
            run_id: None,
            node_id: None,
            role: "user".to_owned(),
            text: "PR merged".to_owned(),
            context: None,
            attachments: Vec::new(),
            streaming: false,
            created_at: "2026-11-22T10:03:00.000Z".to_owned(),
            updated_at: "2026-11-22T10:03:00.000Z".to_owned(),
            delegated_completion: None,
        })),
    };
    proj.handle_and_publish(&hub, &wake_ev, Some(&thread), Some(&wf_meta));

    let items = hub.recent(&MilestoneRecentOptions::default()).events;
    assert_eq!(items.len(), 1);
    assert!(matches!(
        &items[0],
        MilestoneEvent::PrWatchWake {
            watch_ended: true,
            ..
        }
    ));
    assert_eq!(items[0].effective_workflow_run_id(), Some("wf-42"));
    Ok(())
}
