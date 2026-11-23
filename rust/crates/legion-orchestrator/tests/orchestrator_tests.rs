//! Integration and property-based tests for `legion-orchestrator`.

mod orchestrator_tests {
    mod decider_and_rollback;
    mod decider_tests;
    mod helpers;
    mod locks_and_sink;
    mod outbox_and_projections;
    mod proptests;
    mod shell_tests;
    mod wave2_output;
    mod wave2_queue;
    mod wave2_recovery;
    mod wave2_subagents;
    mod wave3_checkpoints_and_schedules;
    mod wave3_delegation_and_subagents;
    mod wave3_helpers;
    mod wave3_upstream_fixes;
    mod wave3_workflow_and_output;
}
