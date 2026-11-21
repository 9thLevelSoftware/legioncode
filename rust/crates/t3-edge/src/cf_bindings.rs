//! Cloudflare Workers host binding models and effect planners for D1, R2, Hyperdrive,
//! Durable Objects, and Worker scheduled cron handlers.
//!
//! All types in this module are pure Safe Rust (`#![forbid(unsafe_code)]`), deterministic with
//! explicit timestamps (`now_ms` / `now_secs`), and compile cleanly on `wasm32-unknown-unknown`.
//!
//! # Examples
//!
//! ```
//! use t3_edge::cf_bindings::{
//!     CLOUDFLARE_UPSTREAM_API_KEY, D1BindingPlan, D1DatabaseBinding, HookInboxDurableActor,
//!     HyperdriveRelayPlan, R2BindingOp, R2BucketBinding, apply_upstream_pause_query_result,
//!     run_scheduled_cron,
//! };
//! use t3_edge::relay_hooks::{CloudflareApiPauseGate, DeliveryOutcome, HeldHook};
//!
//! // D1 plans target "INDEX" and "SAVINGS" with positional parameters.
//! let gc = D1BindingPlan::delete_expired_gc_batch(5_000, 128);
//! assert_eq!(gc.database, D1DatabaseBinding::Index);
//! assert!(gc.sql.contains("DELETE FROM generations"));
//!
//! // R2 UploadPart enforces BUG-30 (`1..=64`).
//! assert!(R2BindingOp::upload_part(R2BucketBinding::Clients, "k", "u1", 65, vec![1]).is_err());
//! assert!(R2BindingOp::upload_part(R2BucketBinding::Clients, "k", "u1", 64, vec![1]).is_ok());
//!
//! // Hyperdrive upstream_api_pauses read errors do NOT advance checked_at_ms (BUG-32).
//! let pause_plan = HyperdriveRelayPlan::read_upstream_api_pause(CLOUDFLARE_UPSTREAM_API_KEY);
//! assert!(pause_plan.sql.contains("relay_upstream_api_pauses"));
//! let mut gate = CloudflareApiPauseGate::new();
//! assert!(apply_upstream_pause_query_result(&mut gate, 1_000, Err("pg timeout")).is_err());
//! assert_eq!(gate.checked_at_ms(), None);
//!
//! // Durable Object actor preserves BUG-29 fencing and tracks alarm_at_ms.
//! let mut actor = HookInboxDurableActor::new("env-1");
//! let hook = HeldHook::new("d1", "h1", 1_000, "POST", b"{}".to_vec());
//! assert_eq!(actor.hold(hook, "https://old.tunnel.test", 1_000), Ok(11_000));
//! let gen1 = actor.target_generation();
//! assert!(actor.wake("https://new.tunnel.test", 1_050));
//! assert_eq!(actor.alarm_at_ms(), Some(1_050));
//! let res = actor.record_delivery_outcome(gen1, "d1", DeliveryOutcome::Unreachable, 9_000);
//! assert!(res.is_ignored_stale());
//!
//! // Scheduled cron plans for "* * * * *" and "0 * * * *".
//! let minutely = run_scheduled_cron("* * * * *", 60_000, 256);
//! assert!(minutely.recognized);
//! let hourly = run_scheduled_cron("0 * * * *", 86_400_000 * 32, 256);
//! assert!(hourly.recognized);
//! ```

pub use crate::cf_bindings_cron::{CronScheduleKind, ScheduledCronReport, run_scheduled_cron};
pub use crate::cf_bindings_d1::{
    D1BindingPlan, D1DatabaseBinding, GC_CLAIM_LEASE_SECONDS, MAX_GC_BATCH_SIZE,
    SAVINGS_RETENTION_MS,
};
pub use crate::cf_bindings_r2::{R2BindingOp, R2BucketBinding};
pub use crate::cf_bindings_relay::{
    CLOUDFLARE_UPSTREAM_API_KEY, HookInboxDurableActor, HyperdriveRelayPlan, HyperdriveTable,
    apply_upstream_pause_query_result,
};
