use std::error::Error;

use legion_contracts::ids::{NodeId, RuntimeRequestId, ThreadId};
use legion_contracts::orchestration::RunStatus;
use legion_protocol::mcp::{
    ParentSubagentRecord, PendingChildRequest, ResponseCapabilityKind, ThreadWaitWakeInput,
    canonical_mcp_tool_name, evaluate_thread_wait_wake, has_branded_mcp_tool_prefix,
    is_branded_mcp_tool, is_dual_branded_mcp_tool, mcp_tool_candidates, present_dual_tool,
    present_mcp_tool_name,
};
use serde_json::json;

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn dual_tool_presentation_and_thread_wait_live_and_first_run_guards() -> TestResult {
    assert_eq!(canonical_mcp_tool_name("t3_thread_wait"), "thread_wait");
    assert_eq!(
        canonical_mcp_tool_name("weather.t3_thread_wait"),
        "thread_wait"
    );
    assert_eq!(
        canonical_mcp_tool_name("mcp__legion_code__t3_thread_read"),
        "thread_read"
    );
    assert!(has_branded_mcp_tool_prefix("t3_thread_wait"));
    assert!(is_branded_mcp_tool("t3_thread_wait", "thread_wait"));
    assert!(is_dual_branded_mcp_tool(
        "legion_thread_wait",
        "thread_wait"
    ));
    assert_eq!(
        present_mcp_tool_name("t3_thread_wait"),
        "legion_thread_wait"
    );
    let pres = present_dual_tool("t3_thread_wait", "Wait for thread", "Waits on a thread.");
    assert_eq!(pres.primary_name, "legion_thread_wait");
    assert_eq!(pres.legacy_alias, "t3_thread_wait");
    let candidates = mcp_tool_candidates("thread_wait");
    assert_eq!(candidates[0], "legion_thread_wait");
    assert!(candidates.contains(&"t3_thread_wait".to_owned()));

    // #40604 + #40633 checks:
    let parent_tid = ThreadId::new("parent-1")?;
    let child_tid = ThreadId::new("child-1")?;
    let task_id = NodeId::new("task-1")?;
    let req_id = RuntimeRequestId::new("req-1")?;
    let mk_input =
        |result: Option<&str>, delegated_id: Option<NodeId>, live_req: bool| ThreadWaitWakeInput {
            parent_thread_id: Some(parent_tid.clone()),
            target_thread_id: child_tid.clone(),
            target_forked_from: Some(json!({ "type": "node", "nodeId": task_id.as_str() })),
            run_delegated_task_id: delegated_id,
            run_status: Some(RunStatus::Running),
            timed_out: true,
            parent_subagents: vec![ParentSubagentRecord {
                id: task_id.clone(),
                thread_id: parent_tid.clone(),
                origin: "app_owned".to_owned(),
                child_thread_id: Some(child_tid.clone()),
                completion_delivery_state: Some("pending".to_owned()),
                result: result.map(str::to_owned),
            }],
            child_requests: vec![PendingChildRequest {
                id: req_id.clone(),
                node_id: None,
                kind: "user_input".to_owned(),
                status: "pending".to_owned(),
                response_capability: if live_req {
                    ResponseCapabilityKind::Live
                } else {
                    ResponseCapabilityKind::Message
                },
            }],
        };
    assert!(evaluate_thread_wait_wake(&mk_input(None, None, false)).wakes_caller);
    assert!(!evaluate_thread_wait_wake(&mk_input(None, None, true)).wakes_caller);
    assert!(!evaluate_thread_wait_wake(&mk_input(Some("done"), None, false)).wakes_caller);
    assert!(
        evaluate_thread_wait_wake(&mk_input(Some("done"), Some(task_id.clone()), false))
            .wakes_caller
    );
    Ok(())
}
