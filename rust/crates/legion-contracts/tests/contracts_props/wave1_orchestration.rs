//! Tests for Wave 1 orchestration extensions, checkpoint rewind helper, and workflow/feedback RPC scopes.

use legion_contracts::ids::{CheckpointId, CheckpointScopeId, NodeId, RunId, ThreadId};
use legion_contracts::orchestration::{
    Checkpoint, ProviderTurnWire, WorkflowOrigin, WorkflowThreadCreateCommand,
    WorkflowThreadLineage, is_delivered_provider_turn, is_listed_thread_lineage,
    repository_rewind_checkpoint,
};
use legion_contracts::permissions::{
    EnvironmentScope, WsMethod, required_scopes_for_rpc, server_required_scope_for_ws_method,
};
use serde_json::json;

#[test]
fn orchestration_extensions_and_rpc_scopes_match_ts() {
    let wf_lineage: WorkflowThreadLineage = serde_json::from_value(json!({
        "parentThreadId": null,
        "relationshipToParent": null,
        "rootThreadId": "th-1",
        "workflowStep": true,
    }))
    .expect("lineage");
    assert!(!is_listed_thread_lineage(&wf_lineage));

    let turn_wire: ProviderTurnWire = serde_json::from_value(json!({
        "id": "pturn-1",
        "providerThreadId": "pt-1",
        "nodeId": "n-1",
        "runAttemptId": null,
        "nativeTurnRef": null,
        "ordinal": 1,
        "status": "failed",
        "startedAt": null,
        "completedAt": null,
        "promptDropped": true,
    }))
    .expect("turn wire");
    assert!(!is_delivered_provider_turn(turn_wire.prompt_dropped));
    let wf_create: WorkflowThreadCreateCommand = serde_json::from_value(json!({
        "createdBy": "system",
        "creationSource": "server",
        "commandId": "cmd-wf",
        "threadId": "th-wf",
        "projectId": "proj-1",
        "title": "Step",
        "titleRenamed": false,
        "modelSelection": { "instanceId": "codex", "model": "gpt-5" },
        "runtimeMode": "full-access",
        "interactionMode": "default",
        "branch": null,
        "worktreePath": null,
        "workflowRun": {
            "environmentId": "env-1",
            "runId": "wrun-1",
            "workflow": "triage",
            "step": "inspect"
        }
    }))
    .expect("wf create");
    assert_eq!(wf_create.title_renamed, Some(false));
    assert!(matches!(
        wf_create.workflow_run,
        Some(WorkflowOrigin { .. })
    ));

    let make_cp = |id: &str, ord: u64, run: Option<&str>, status: &str| Checkpoint {
        id: CheckpointId::new(id).expect("cp"),
        thread_id: ThreadId::new("th-1").expect("th"),
        scope_id: CheckpointScopeId::new("sc-1").expect("sc"),
        run_id: run.map(|r| RunId::new(r).expect("run")),
        node_id: NodeId::new("n-1").expect("node"),
        parent_checkpoint_id: None,
        ordinal_within_scope: ord,
        app_run_ordinal: Some(ord),
        r#ref: "refs/legioncode/cp".parse().expect("ref"),
        status: status.to_owned(),
        files: vec![],
        files_total: None,
        captured_at: "2026-11-22T00:00:00Z".to_owned(),
    };
    let cps = [
        make_cp("c2-base", 2, None, "ready"),
        make_cp("c2", 2, Some("r2"), "ready"),
    ];
    let rewound = repository_rewind_checkpoint(&cps, 2).expect("rewind target");
    assert_eq!(rewound.id.as_str(), "c2-base");
    let unrecorded = [
        make_cp("c2-err", 2, Some("r2"), "error"),
        make_cp("c3-base", 3, None, "ready"),
    ];
    assert!(repository_rewind_checkpoint(&unrecorded, 1).is_none());

    assert_eq!(
        required_scopes_for_rpc(&WsMethod::WorkflowsAnswer, None).expect("scope"),
        vec![EnvironmentScope::OrchestrationOperate]
    );
    assert_eq!(
        required_scopes_for_rpc(&WsMethod::WorkflowsSaveDefinition, None).expect("scope"),
        vec![EnvironmentScope::TerminalOperate]
    );
    assert_eq!(
        required_scopes_for_rpc(&WsMethod::AgentFeedbackAnswerRequest, None).expect("scope"),
        vec![EnvironmentScope::OrchestrationOperate]
    );
    assert_eq!(
        server_required_scope_for_ws_method(&WsMethod::WorkflowsSubscribe),
        Some(EnvironmentScope::OrchestrationRead)
    );
}
