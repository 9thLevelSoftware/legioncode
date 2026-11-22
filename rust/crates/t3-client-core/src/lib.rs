#![forbid(unsafe_code)]
//! Target-agnostic client state machine: reconnect backoff, probes, `DPoP` renewal, steady shells,
//! thread family caching, `SoA` diff parsing, and math/Windows-path-safe Markdown scanning.
//!
//! Every module is pure Safe Rust with explicit timestamps (`now_ms: u64`) and zero `tokio::spawn`
//! or blocking `std::thread` / `std::fs` dependencies, allowing identical execution on native
//! `UniFFI` targets and `wasm32-unknown-unknown`.
//!
//! # Examples
//!
//! ```
//! use t3_client_core::backoff::{FailureReason, ReconnectBackoff};
//! use t3_client_core::dpop_renewal::next_renewal_delay_ms;
//! use t3_client_core::probe::{ConnectionRouteKind, ConnectionWakeup, ProbeSignal, probe_timeout_for};
//!
//! let mut backoff = ReconnectBackoff::new();
//! let loss = backoff.record_failure(FailureReason::Transient, true, 1.0);
//! assert_eq!(loss.delay_ms, 250);
//! assert_eq!(
//!     probe_timeout_for(
//!         ProbeSignal::Wakeup(ConnectionWakeup::ApplicationActiveProbe),
//!         Some(ConnectionRouteKind::Loopback),
//!     ),
//!     Some(3_000)
//! );
//! assert_eq!(next_renewal_delay_ms(120_000, 10_000, false), Some(50_000));
//! ```

mod diff_hunk;
mod diff_parser;
mod diff_soa;
mod diff_word;
mod linked_threads_batch;
mod linked_threads_repos;
mod linked_threads_worktree;
mod markdown_blocks;
mod markdown_inline_links;
mod markdown_math_scan;
mod markdown_scanner;
mod probe_machine;
mod thread_families_cache;

pub mod backoff;
pub mod client_caches;
pub mod diff_markdown;
pub mod dpop_renewal;
pub mod linked_threads;
pub mod probe;
pub mod steady_shells;
pub mod steady_shells_resume;
pub mod supervisor_recovery;
pub mod thread_families;
