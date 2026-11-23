use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use legion_contracts::ids::{ProjectId, RunId, ThreadId};
use legion_protocol::milestones::{
    MILESTONE_IDS_KEPT, MILESTONE_KEPT, MILESTONE_SUBSCRIBER_BACKLOG, McpEventStreamItem,
    MilestoneActor, MilestoneActorType, MilestoneEnvelope, MilestoneEvent,
    MilestoneEventPollRequest, MilestoneEventType, MilestoneEvents, MilestoneFilter,
    MilestoneFollowItem, MilestoneHub, MilestoneId, MilestoneReadOptions, MilestoneRecentOptions,
    MilestoneRunOutcome, MilestoneThreadFilterArgs, MilestoneWorkflowFilterArgs,
    MilestoneWorkflowOutcome, WorkflowRunFields, hidden_from_threads, milestone_matches,
    parse_iso_epoch_ms,
};

type TestResult = Result<(), Box<dyn Error>>;

fn make_run_milestone(
    idx: usize,
    is_subagent: bool,
    occurred: &str,
    wf: Option<&str>,
) -> Result<MilestoneEvent, Box<dyn Error>> {
    Ok(MilestoneEvent::ThreadRunSettled {
        envelope: MilestoneEnvelope {
            id: MilestoneId::new(format!("m:{idx}"))?,
            occurred_at: occurred.to_owned(),
            project_id: Some(ProjectId::new("proj-1")?),
            thread_id: Some(ThreadId::new("t-1")?),
            parent_thread_id: None,
            actor: MilestoneActor::new(MilestoneActorType::Agent, None, None),
            is_subagent_child: is_subagent,
            workflow_run_id: wf.map(str::to_owned),
        },
        run_id: RunId::new(format!("run-{idx}"))?,
        outcome: MilestoneRunOutcome::Completed,
    })
}

#[test]
fn milestone_hub_filters_dedups_and_ages_out() -> TestResult {
    let clock_ms = Arc::new(AtomicI64::new(
        parse_iso_epoch_ms("2026-11-22T12:00:00.000Z").ok_or("bad ts")?,
    ));
    let clock_clone = Arc::clone(&clock_ms);
    let hub = MilestoneHub::with_clock(
        1_700_000_000_000,
        Arc::new(move || clock_clone.load(Ordering::Relaxed)),
    );
    let events_api = MilestoneEvents::new(hub.clone());
    assert_eq!(events_api.threads().len(), 6);
    assert_eq!(events_api.workflows().len(), 6);
    assert!(hidden_from_threads(false));
    assert!(!hidden_from_threads(true));

    let e1 = make_run_milestone(1, false, "2026-11-22T11:59:00.000Z", Some("wf-run-1"))?;
    let e2_sub = make_run_milestone(2, true, "2026-11-22T11:59:30.000Z", None)?;
    let e3_wf = MilestoneEvent::WorkflowRunEnded {
        envelope: MilestoneEnvelope {
            id: MilestoneId::new("wf:1")?,
            occurred_at: "2026-11-22T11:59:50.000Z".to_owned(),
            project_id: None,
            thread_id: None,
            parent_thread_id: None,
            actor: MilestoneActor::new(MilestoneActorType::System, None, None),
            is_subagent_child: false,
            workflow_run_id: None,
        },
        workflow_fields: WorkflowRunFields {
            workflow_run_id: "wf-run-1".to_owned(),
            workflow: "deploy".to_owned(),
            run_title: "Deploy prod".to_owned(),
            parent_workflow_run_id: None,
        },
        outcome: MilestoneWorkflowOutcome::Completed,
        detail: None,
    };
    hub.publish(e1.clone());
    hub.publish(e1.clone());
    hub.publish(e2_sub);
    hub.publish(e3_wf.clone());

    assert!(milestone_matches(
        &e1,
        Some(&MilestoneFilter {
            workflow_run_ids: Some(vec!["wf-run-1".to_owned()]),
            ..MilestoneFilter::default()
        })
    ));
    assert!(milestone_matches(
        &e3_wf,
        Some(&MilestoneFilter {
            workflows: Some(vec!["deploy".to_owned()]),
            ..MilestoneFilter::default()
        })
    ));

    let polled = events_api.poll_thread(
        MilestoneEventType::ThreadRunSettled,
        MilestoneThreadFilterArgs {
            project_ids: Some(vec![ProjectId::new("proj-1")?]),
            ..MilestoneThreadFilterArgs::default()
        },
        MilestoneEventPollRequest {
            cursor: Some(format!("{}.0", hub.epoch())),
            max_age_ms: Some(40_000),
            max_events: None,
        },
    );
    assert!(polled.truncated);
    assert_eq!(polled.events.len(), 0);

    let recent_all = hub.recent(&MilestoneRecentOptions {
        filter: Some(MilestoneFilter {
            include_subagent_children: Some(true),
            ..MilestoneFilter::default()
        }),
        ..MilestoneRecentOptions::default()
    });
    assert_eq!(recent_all.events.len(), 3);
    Ok(())
}

#[test]
fn milestone_hub_backlog_overflow_and_ring_dedup_capacity() -> TestResult {
    let hub = MilestoneHub::new();
    let events_api = MilestoneEvents::new(hub.clone());
    let mut follower = events_api.stream_workflow(
        MilestoneEventType::WorkflowRunStarted,
        MilestoneWorkflowFilterArgs::default(),
        &MilestoneEventPollRequest::default(),
    );
    for idx in 10..(10 + MILESTONE_SUBSCRIBER_BACKLOG + 5) {
        hub.publish(make_run_milestone(
            idx,
            false,
            "2026-11-22T12:00:00.000Z",
            None,
        )?);
    }
    let first = follower.try_next().ok_or("expected lagged gap item")?;
    assert!(matches!(first, MilestoneFollowItem::Gap { .. }));
    assert!(matches!(
        McpEventStreamItem::from_follow_item(first),
        McpEventStreamItem::Gap { .. }
    ));

    for idx in 2_000..(2_000 + MILESTONE_KEPT + 20) {
        hub.publish(make_run_milestone(
            idx,
            false,
            "2026-11-22T12:00:00.000Z",
            None,
        )?);
    }
    assert_eq!(MILESTONE_IDS_KEPT, 4_000);
    let page = hub.read(&MilestoneReadOptions::default(), 5);
    assert_eq!(page.events.len(), 0);
    Ok(())
}
