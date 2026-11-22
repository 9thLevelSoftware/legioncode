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
#[path = "contracts_props/policy_templates.rs"]
mod policy_templates;
