#![forbid(unsafe_code)]
//! Built-in versioned `Spec` stage job (`workflowBoard.spec@1`) implementing [`WorkflowBodyDriver`].

use std::sync::Arc;

use crate::engine::{WorkflowBodyDriver, WorkflowDefinition};
use crate::engine_replay::ReplayContext;
use crate::engine_step::PassOutcome;
use crate::stage_driver_common::{StageTurnSpec, run_stage_turn_pass, stage_payload_schema};
use crate::traits::BoxFuture;

/// Built-in workflow name for Project Workflow Board `Spec` stage execution.
pub const WORKFLOW_BOARD_SPEC_NAME: &str = "workflowBoard.spec";
/// Built-in version for [`WORKFLOW_BOARD_SPEC_NAME`].
pub const WORKFLOW_BOARD_SPEC_VERSION: i64 = 1;

/// Native Rust [`WorkflowBodyDriver`] for `workflowBoard.spec@1`.
#[derive(Debug, Default, Clone, Copy)]
pub struct SpecStageDriver;

impl WorkflowBodyDriver for SpecStageDriver {
    fn run_pass<'a>(&'a self, ctx: &'a mut ReplayContext) -> BoxFuture<'a, PassOutcome> {
        Box::pin(async move {
            run_stage_turn_pass(
                ctx,
                StageTurnSpec {
                    stage_name: WORKFLOW_BOARD_SPEC_NAME,
                    step_id: "spec",
                    wait_key: "await_spec_completion",
                    title_prefix: "Spec",
                    lead_line: "Author or refine the specification",
                    spec_label: "Spec draft file",
                    brief_label: "Spec brief file",
                },
            )
            .await
        })
    }
}

pub(crate) fn spec_stage_definition() -> WorkflowDefinition {
    WorkflowDefinition {
        name: WORKFLOW_BOARD_SPEC_NAME.to_owned(),
        version: WORKFLOW_BOARD_SPEC_VERSION,
        payload_schema: stage_payload_schema(),
        returns_schema: None,
        resume_after_ms: Vec::new(),
        triggers: Vec::new(),
        has_on_end: false,
        driver: Arc::new(SpecStageDriver),
    }
}
