//! Integration and property-based tests for `legion-contracts` (`ids`, `environment`,
//! `policy`, `id_table`, `pull_requests`, `orchestration`, `permissions`) and BUG-17..20.

#[path = "contracts_props/brand.rs"]
mod brand;
#[path = "contracts_props/id_table_pr.rs"]
mod id_table_pr;
#[path = "contracts_props/ids_env.rs"]
mod ids_env;
#[path = "contracts_props/orchestration_stream.rs"]
mod orchestration_stream;
#[path = "contracts_props/permissions_guards.rs"]
mod permissions_guards;
#[path = "contracts_props/permissions_m3_guards.rs"]
mod permissions_m3_guards;
#[path = "contracts_props/permissions_m4_guards.rs"]
mod permissions_m4_guards;
#[path = "contracts_props/policy_templates.rs"]
mod policy_templates;
#[path = "contracts_props/wave1_contracts.rs"]
mod wave1_contracts;
#[path = "contracts_props/wave1_orchestration.rs"]
mod wave1_orchestration;
#[path = "contracts_props/workflow_board.rs"]
mod workflow_board;
#[path = "contracts_props/workflow_board_fixtures.rs"]
mod workflow_board_fixtures;
#[path = "contracts_props/workflow_board_m2.rs"]
mod workflow_board_m2;
#[path = "contracts_props/workflow_board_m2_parity.rs"]
mod workflow_board_m2_parity;
#[path = "contracts_props/workflow_board_m3.rs"]
mod workflow_board_m3;
#[path = "contracts_props/workflow_board_m3_parity.rs"]
mod workflow_board_m3_parity;
#[path = "contracts_props/workflow_board_m4.rs"]
mod workflow_board_m4;
#[path = "contracts_props/workflow_board_optional_null_parity.rs"]
mod workflow_board_optional_null_parity;
