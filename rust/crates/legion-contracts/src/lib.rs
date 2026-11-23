#![forbid(unsafe_code)]
//! Wire-compatible domain contracts, policy schemas, and RPC authorization scopes.
//!
//! `legion-contracts` mirrors `LegionCode`'s TypeScript `@legioncode/contracts` package with
//! strict Safe Rust validation, preserving protocol v2 JSON wire compatibility
//! while eliminating runtime defects around permissions, unknown stream event
//! tags, legacy pull-request unlinking, and ID-table expansion.

pub mod brand;
pub mod environment;
pub mod id_table;
pub mod ids;
pub mod ipc;
pub mod mcp_sign_in;
pub mod orchestration;
pub mod peer_links;
pub mod permissions;
pub mod policy;
pub mod pull_requests;
pub mod secrets;
pub mod workspace_folders;
