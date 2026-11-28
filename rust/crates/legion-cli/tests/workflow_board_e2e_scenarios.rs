#![forbid(unsafe_code)]
//! E2E-M5 Tier 3 pairwise and Tier 4 real-world scenario tests for Project Workflow Board.

#[expect(
    dead_code,
    reason = "shared E2E harness includes helpers used across other E2E binaries"
)]
#[path = "workflow_board_e2e/mod.rs"]
mod workflow_board_e2e;

#[path = "workflow_board_e2e_scenarios/mod.rs"]
mod scenarios;
