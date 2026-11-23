//! Unit tests for ported Wave 1 Rust contracts (`milestones`, `workflows`, `agent_feedback`, `issue`, `orchestration` & `permissions`).

use std::collections::BTreeMap;

use legion_contracts::agent_feedback::{
    AgentFeedbackAnswerChoice, AgentFeedbackAnswerInput, AgentFeedbackFailureReason,
    AgentFeedbackKind, AgentFeedbackPreview, AgentFeedbackRequestError, AgentFeedbackRequestStatus,
    AgentFeedbackTrace, AgentFeedbackTraceEntry, AgentFeedbackTraceEntryType,
    AgentFeedbackTraceOutcome,
};
use legion_contracts::brand::{LEGION_CODE, canonical_mcp_tool_suffix};
use legion_contracts::ids::{
    IssueTrackerAccountId, MilestoneId, ProjectId, RunId, ThreadId, TurnItemId,
};
use legion_contracts::ipc::{DesktopUpdateChannel, DesktopUpdateTrackMismatch};
use legion_contracts::issue::{
    ISSUE_LIST_MAX_EXCLUDED_PROJECTS, IssueEventReference, IssueEventReferenceKind,
    IssueTrackerBindProjectInput, IssueTrackerConnectionList, IssueTrackerProjectBinding,
    MAX_ISSUE_TRACKER_ADDITIONAL_SCOPES, issue_tracker_binding_scopes,
};
use legion_contracts::milestones::{
    MilestoneActor, MilestoneActorType, MilestoneEvent, MilestoneEventKind, MilestoneEventType,
    MilestoneFilter, MilestoneThreadRunOutcome, MilestoneWorkflowOutcome,
    MilestoneWorkflowRunFields, milestone_matches,
};
use legion_contracts::orchestration::{TurnItem, TurnItemKind, is_known_turn_item_type};
use legion_contracts::peer_links::PeerLinkRemoveResult;
use legion_contracts::policy::list_manageable_settings;
use legion_contracts::workflows::{
    WORKFLOW_REFERENCE_KIND_ISSUE, WorkflowAnswerInput, WorkflowCounts, WorkflowDefinitionFiles,
    WorkflowRequestError, WorkflowRun, WorkflowRunDetail, WorkflowRunStatus, WorkflowStep,
    WorkflowStepKind, WorkflowStepStatus,
};
use serde_json::json;

#[test]
fn dual_mcp_prefixes_and_track_mismatch_and_settings_match_ts() {
    assert_eq!(
        canonical_mcp_tool_suffix("mcp__t3_code__t3_submit_output"),
        "submit_output"
    );
    assert_eq!(
        canonical_mcp_tool_suffix("mcp__t3-code__t3_thread_wait"),
        "thread_wait"
    );
    assert_eq!(
        canonical_mcp_tool_suffix("t3-code_t3_workflow_stop"),
        "workflow_stop"
    );
    assert_eq!(
        LEGION_CODE.canonical_mcp_tool_suffix("mcp__legion_code__legion_workflow_stop"),
        "workflow_stop"
    );

    let remove = PeerLinkRemoveResult {
        removed: true,
        ended_there: Some(false),
    };
    let remove_json = serde_json::to_value(remove).expect("serialize remove");
    assert_eq!(remove_json, json!({"removed": true, "endedThere": false}));

    let mismatch = DesktopUpdateTrackMismatch {
        build_channel: DesktopUpdateChannel::Latest,
    };
    assert_eq!(
        serde_json::to_value(mismatch).expect("mismatch json"),
        json!({"buildChannel": "latest"})
    );

    let settings = list_manageable_settings();
    assert!(
        settings
            .iter()
            .any(|(p, v)| *p == "agentFeedback.mode" && *v == Some(json!("off")))
    );
    assert!(
        settings
            .iter()
            .any(|(p, v)| *p == "workflowsEnabled" && *v == Some(json!(false)))
    );
}

#[test]
fn milestones_matching_and_round_trip() {
    let wf_event = MilestoneEvent {
        id: MilestoneId::new("ms-1").expect("ms id"),
        occurred_at: "2026-11-22T00:00:00Z".to_owned(),
        project_id: None,
        thread_id: None,
        parent_thread_id: None,
        actor: MilestoneActor {
            actor_type: MilestoneActorType::System,
            id: None,
            label: None,
        },
        is_subagent_child: false,
        kind: MilestoneEventKind::WorkflowRunEnded {
            workflow_run: MilestoneWorkflowRunFields {
                workflow_run_id: "wrun-1".to_owned(),
                workflow: "triage".to_owned(),
                run_title: "Triage #42".to_owned(),
                parent_workflow_run_id: None,
            },
            outcome: MilestoneWorkflowOutcome::Completed,
            detail: None,
        },
    };
    let encoded = serde_json::to_value(&wf_event).expect("encode milestone");
    assert_eq!(encoded["type"], "workflow.run_ended");
    assert_eq!(
        serde_json::from_value::<MilestoneEvent>(encoded).expect("decode milestone"),
        wf_event
    );
    assert!(milestone_matches(&wf_event, &MilestoneFilter::default()));
    assert!(milestone_matches(
        &wf_event,
        &MilestoneFilter {
            types: Some(vec![MilestoneEventType::WorkflowRunEnded]),
            workflows: Some(vec!["triage".to_owned()]),
            workflow_run_ids: Some(vec!["wrun-1".to_owned()]),
            ..Default::default()
        }
    ));

    let subagent_event = MilestoneEvent {
        id: MilestoneId::new("ms-2").expect("ms id"),
        occurred_at: "2026-11-22T00:00:01Z".to_owned(),
        project_id: Some(ProjectId::new("proj-1").expect("proj")),
        thread_id: Some(ThreadId::new("th-child").expect("th")),
        parent_thread_id: Some(ThreadId::new("th-root").expect("th")),
        actor: MilestoneActor {
            actor_type: MilestoneActorType::Agent,
            id: Some("codex".to_owned()),
            label: None,
        },
        is_subagent_child: true,
        kind: MilestoneEventKind::ThreadRunSettled {
            workflow_run_id: None,
            run_id: RunId::new("run-1").expect("run"),
            outcome: MilestoneThreadRunOutcome::Completed,
        },
    };
    assert!(!milestone_matches(
        &subagent_event,
        &MilestoneFilter::default()
    ));
    assert!(milestone_matches(
        &subagent_event,
        &MilestoneFilter {
            include_subagent_children: Some(true),
            ..Default::default()
        }
    ));
}

#[test]
fn workflows_contracts_round_trip() {
    assert_eq!(WORKFLOW_REFERENCE_KIND_ISSUE, "issue");
    let detail = WorkflowRunDetail {
        run: WorkflowRun {
            run_id: "wr-1".to_owned(),
            workflow: "review".to_owned(),
            version: 1,
            title: "PR #7".to_owned(),
            status: WorkflowRunStatus::Waiting,
            detail: Some("waiting for answer".to_owned()),
            result: None,
            pauses: 0,
            retired: false,
            resumes_at: None,
            parent_run_id: None,
            parent_step: None,
            started_at: "2026-11-22T00:00:00Z".to_owned(),
            updated_at: "2026-11-22T00:00:01Z".to_owned(),
            ended_at: None,
        },
        steps: vec![WorkflowStep {
            name: "ask_user".to_owned(),
            kind: WorkflowStepKind::Ask,
            status: WorkflowStepStatus::Running,
            attempts: 1,
            detail: None,
            thread_id: None,
            environment_id: None,
            result: None,
            said: None,
            started_at: "2026-11-22T00:00:00Z".to_owned(),
            ended_at: None,
        }],
        references: vec![],
    };
    let encoded_detail = serde_json::to_value(&detail).expect("workflow detail");
    assert_eq!(
        serde_json::from_value::<WorkflowRunDetail>(encoded_detail).expect("decode detail"),
        detail
    );
    let counts = WorkflowCounts::default();
    assert_eq!(counts.total, 0);
    let defs = WorkflowDefinitionFiles {
        directory: "/wf".to_owned(),
        single: false,
        files: vec![],
        problems: vec![],
        revision: 1,
        problem: None,
    };
    assert_eq!(defs.revision, 1);
    let ans = WorkflowAnswerInput {
        run_id: "wr-1".to_owned(),
        name: "ask".to_owned(),
        choice: "ok".to_owned(),
        reason: None,
    };
    assert_eq!(ans.choice, "ok");
    let err = WorkflowRequestError {
        message: "bad".to_owned(),
    };
    assert_eq!(err.to_string(), "bad");
}

#[test]
fn agent_feedback_and_issue_contracts_round_trip() {
    let preview = AgentFeedbackPreview {
        kind: AgentFeedbackKind::Confusing,
        subject: "Tool error".to_owned(),
        expected: "Success".to_owned(),
        actual: "Timeout".to_owned(),
        workaround: "Retry".to_owned(),
        trace: Some(AgentFeedbackTrace {
            turns: 1,
            text_turns: 1,
            truncated: false,
            entries: vec![AgentFeedbackTraceEntry {
                turn: 1,
                entry_type: AgentFeedbackTraceEntryType::Tool,
                tool: Some("read".to_owned()),
                outcome: Some(AgentFeedbackTraceOutcome::Error),
                duration_ms: Some(15),
                length: None,
                text: None,
            }],
        }),
    };
    assert_eq!(preview.kind, AgentFeedbackKind::Confusing);

    let feedback_item = json!({
        "id": "ti-fb-1",
        "threadId": "th-1",
        "runId": "run-1",
        "nodeId": "node-1",
        "providerThreadId": null,
        "providerTurnId": null,
        "nativeItemRef": null,
        "parentItemId": null,
        "ordinal": 1,
        "status": "waiting",
        "title": null,
        "startedAt": null,
        "completedAt": null,
        "updatedAt": "2026-11-22T00:00:00Z",
        "type": "feedback_request",
        "preview": preview,
        "feedbackStatus": AgentFeedbackRequestStatus::Pending,
    });
    assert!(is_known_turn_item_type("feedback_request"));
    let parsed_item: TurnItem = serde_json::from_value(feedback_item).expect("feedback item");
    assert!(matches!(
        parsed_item.kind,
        TurnItemKind::FeedbackRequest { .. }
    ));

    let fb_ans = AgentFeedbackAnswerInput {
        thread_id: ThreadId::new("th-1").expect("th"),
        turn_item_id: TurnItemId::new("ti-fb-1").expect("ti"),
        answer: AgentFeedbackAnswerChoice::Send,
        include_conversation: Some(true),
    };
    assert_eq!(fb_ans.answer, AgentFeedbackAnswerChoice::Send);
    let fb_err = AgentFeedbackRequestError {
        reason: AgentFeedbackFailureReason::FeedbackOff,
    };
    assert_eq!(
        fb_err.to_string(),
        "Agent feedback is off, so this report was not sent."
    );

    assert_eq!(ISSUE_LIST_MAX_EXCLUDED_PROJECTS, 1_000);
    assert_eq!(MAX_ISSUE_TRACKER_ADDITIONAL_SCOPES, 250);
    let acc_id = IssueTrackerAccountId::new("1-work").expect("account id");
    assert!(IssueTrackerAccountId::new("BadUpper").is_err());
    let binding = IssueTrackerProjectBinding {
        tracker: "linear".parse().expect("tracker"),
        account_id: Some(acc_id.clone()),
        scope: "ENG".to_owned(),
        additional_scopes: Some(vec!["INFRA".to_owned()]),
    };
    assert_eq!(issue_tracker_binding_scopes(&binding), vec!["ENG", "INFRA"]);
    let ev_ref = IssueEventReference {
        repository: "org/repo".to_owned(),
        number: 9,
        kind: IssueEventReferenceKind::PullRequest,
        url: "https://github.com/org/repo/pull/9".to_owned(),
    };
    assert_eq!(ev_ref.number, 9);
    let conn_list = IssueTrackerConnectionList {
        trackers: vec![],
        accounts: Some(vec![]),
        bindings: BTreeMap::new(),
        account_bindings: Some(BTreeMap::new()),
    };
    assert!(conn_list.accounts.is_some());
    let bind_in = IssueTrackerBindProjectInput {
        project_id: ProjectId::new("proj-1").expect("proj"),
        binding: None,
        account_id: Some(acc_id),
    };
    assert!(bind_in.binding.is_none());
}
