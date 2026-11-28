#![forbid(unsafe_code)]
//! Built-in versioned fresh-thread `Review` stage job (`workflowBoard.review@1`) implementing [`WorkflowBodyDriver`].

use std::sync::Arc;

use crate::engine::{WorkflowBodyDriver, WorkflowDefinition};
use crate::engine_replay::ReplayContext;
use crate::engine_step::PassOutcome;
use crate::stage_driver_common::{StageTurnSpec, run_stage_turn_pass, stage_payload_schema};
use crate::traits::BoxFuture;

/// Built-in workflow name for Project Workflow Board independent `Review` stage execution.
pub const WORKFLOW_BOARD_REVIEW_NAME: &str = "workflowBoard.review";
/// Built-in version for [`WORKFLOW_BOARD_REVIEW_NAME`].
pub const WORKFLOW_BOARD_REVIEW_VERSION: i64 = 1;

/// Native Rust [`WorkflowBodyDriver`] for `workflowBoard.review@1`.
#[derive(Debug, Default, Clone, Copy)]
pub struct ReviewStageDriver;

impl WorkflowBodyDriver for ReviewStageDriver {
    fn run_pass<'a>(&'a self, ctx: &'a mut ReplayContext) -> BoxFuture<'a, PassOutcome> {
        Box::pin(async move {
            run_stage_turn_pass(
                ctx,
                StageTurnSpec {
                    stage_name: WORKFLOW_BOARD_REVIEW_NAME,
                    step_id: "review",
                    wait_key: "await_review_completion",
                    title_prefix: "Review",
                    lead_line: "Perform an independent fresh-thread review without modifying source files",
                    spec_label: "Approved spec file",
                    brief_label: "Review brief file",
                },
            )
            .await
        })
    }
}

pub(crate) fn review_stage_definition() -> WorkflowDefinition {
    WorkflowDefinition {
        name: WORKFLOW_BOARD_REVIEW_NAME.to_owned(),
        version: WORKFLOW_BOARD_REVIEW_VERSION,
        payload_schema: stage_payload_schema(),
        returns_schema: None,
        resume_after_ms: Vec::new(),
        triggers: Vec::new(),
        has_on_end: false,
        driver: Arc::new(ReviewStageDriver),
    }
}
