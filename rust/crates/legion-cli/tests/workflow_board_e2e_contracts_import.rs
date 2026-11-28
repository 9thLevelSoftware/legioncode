#![forbid(unsafe_code)]
//! E2E-M4 opaque-box Rust tests for F22 (contracts/capabilities/WS auth/MCP) and F24 (`.legion-cli` import).

#[expect(
    dead_code,
    reason = "shared E2E harness includes M1-M3 helpers unused in E2E-M4 contracts/import tests"
)]
#[path = "workflow_board_e2e/mod.rs"]
mod workflow_board_e2e;

#[path = "workflow_board_e2e_contracts_import/f22_t1_tests.rs"]
mod f22_t1_tests;
#[path = "workflow_board_e2e_contracts_import/f22_t2_tests.rs"]
mod f22_t2_tests;
#[path = "workflow_board_e2e_contracts_import/f24_tests.rs"]
mod f24_tests;
#[path = "workflow_board_e2e_contracts_import/helpers.rs"]
mod helpers;
