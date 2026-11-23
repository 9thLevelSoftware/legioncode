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
}
