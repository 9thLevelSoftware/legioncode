#![forbid(unsafe_code)]
//! Built-in versioned `Plan` stage job (`workflowBoard.plan@1`) implementing [`WorkflowBodyDriver`].

use std::sync::Arc;

use crate::engine::{WorkflowBodyDriver, WorkflowDefinition};
use crate::engine_replay::ReplayContext;
use crate::engine_step::PassOutcome;
use crate::stage_driver_common::{StageTurnSpec, run_stage_turn_pass, stage_payload_schema};
use crate::stage_execute::execute_stage_definition;
use crate::stage_review::review_stage_definition;
use crate::stage_spec::spec_stage_definition;
use crate::traits::BoxFuture;

/// Built-in workflow name for Project Workflow Board `Plan` stage execution.
pub const WORKFLOW_BOARD_PLAN_NAME: &str = "workflowBoard.plan";
/// Built-in version for [`WORKFLOW_BOARD_PLAN_NAME`].
pub const WORKFLOW_BOARD_PLAN_VERSION: i64 = 1;

/// Native Rust [`WorkflowBodyDriver`] for `workflowBoard.plan@1`.
#[derive(Debug, Default, Clone, Copy)]
pub struct PlanStageDriver;

impl WorkflowBodyDriver for PlanStageDriver {
    fn run_pass<'a>(&'a self, ctx: &'a mut ReplayContext) -> BoxFuture<'a, PassOutcome> {
        Box::pin(async move {
            run_stage_turn_pass(
                ctx,
                StageTurnSpec {
                    stage_name: WORKFLOW_BOARD_PLAN_NAME,
                    step_id: "plan",
                    wait_key: "await_plan_completion",
                    title_prefix: "Plan",
                    lead_line: "Prepare the implementation plan",
                    spec_label: "Approved spec file",
                    brief_label: "Plan brief file",
                },
            )
            .await
        })
    }
}

/// Returns the built-in native Rust workflow definitions for Project Workflow Board stage jobs.
#[must_use]
pub fn builtin_stage_definitions() -> Vec<WorkflowDefinition> {
    vec![
        spec_stage_definition(),
        WorkflowDefinition {
            name: WORKFLOW_BOARD_PLAN_NAME.to_owned(),
            version: WORKFLOW_BOARD_PLAN_VERSION,
            payload_schema: stage_payload_schema(),
            returns_schema: None,
            resume_after_ms: Vec::new(),
            triggers: Vec::new(),
            has_on_end: false,
            driver: Arc::new(PlanStageDriver),
        },
        execute_stage_definition(),
        review_stage_definition(),
    ]
}
