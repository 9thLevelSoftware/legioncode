#![forbid(unsafe_code)]
//! Built-in versioned `Execute` stage job (`workflowBoard.execute@1`) implementing [`WorkflowBodyDriver`].

use std::sync::Arc;

use crate::engine::{WorkflowBodyDriver, WorkflowDefinition};
use crate::engine_replay::ReplayContext;
use crate::engine_step::PassOutcome;
use crate::stage_driver_common::{StageTurnSpec, run_stage_turn_pass, stage_payload_schema};
use crate::traits::BoxFuture;

/// Built-in workflow name for Project Workflow Board `Execute` task execution.
pub const WORKFLOW_BOARD_EXECUTE_NAME: &str = "workflowBoard.execute";
/// Built-in version for [`WORKFLOW_BOARD_EXECUTE_NAME`].
pub const WORKFLOW_BOARD_EXECUTE_VERSION: i64 = 1;

/// Native Rust [`WorkflowBodyDriver`] for `workflowBoard.execute@1`.
#[derive(Debug, Default, Clone, Copy)]
pub struct ExecuteStageDriver;

impl WorkflowBodyDriver for ExecuteStageDriver {
    fn run_pass<'a>(&'a self, ctx: &'a mut ReplayContext) -> BoxFuture<'a, PassOutcome> {
        Box::pin(async move {
            run_stage_turn_pass(
                ctx,
                StageTurnSpec {
                    stage_name: WORKFLOW_BOARD_EXECUTE_NAME,
                    step_id: "execute",
                    wait_key: "await_execute_completion",
                    title_prefix: "Execute",
                    lead_line: "Execute the assigned task contract inside the card worktree",
                    spec_label: "Approved spec file",
                    brief_label: "Execute brief file",
                },
            )
            .await
        })
    }
}

pub(crate) fn execute_stage_definition() -> WorkflowDefinition {
    WorkflowDefinition {
        name: WORKFLOW_BOARD_EXECUTE_NAME.to_owned(),
        version: WORKFLOW_BOARD_EXECUTE_VERSION,
        payload_schema: stage_payload_schema(),
        returns_schema: None,
        resume_after_ms: Vec::new(),
        triggers: Vec::new(),
        has_on_end: false,
        driver: Arc::new(ExecuteStageDriver),
    }
}
