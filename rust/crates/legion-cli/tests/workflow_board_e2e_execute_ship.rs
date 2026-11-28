#![forbid(unsafe_code)]
//! Opaque-box E2E Tier 1 & Tier 2 test suite for Features F13–F21 (Execute & Ship).

#[expect(
    dead_code,
    reason = "shared E2E harness includes M4 CLI import helpers used in other test targets"
)]
#[path = "workflow_board_e2e/mod.rs"]
mod workflow_board_e2e;

#[path = "workflow_board_e2e_execute_ship/mod.rs"]
mod execute_ship;
