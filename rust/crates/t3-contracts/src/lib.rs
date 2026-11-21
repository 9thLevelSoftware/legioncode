#![forbid(unsafe_code)]
//! Wire-compatible domain contracts, policy schemas, and RPC authorization scopes.
//!
//! `t3-contracts` mirrors T3 Code's TypeScript `@t3tools/contracts` package with
//! strict Safe Rust validation, preserving protocol v2 JSON wire compatibility
//! while eliminating runtime defects around permissions, unknown stream event
//! tags, legacy pull-request unlinking, and ID-table expansion.

pub mod environment;
pub mod id_table;
pub mod ids;
pub mod orchestration;
pub mod permissions;
pub mod policy;
pub mod pull_requests;
