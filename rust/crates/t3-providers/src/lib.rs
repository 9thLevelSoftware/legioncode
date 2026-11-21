#![forbid(unsafe_code)]
//! Provider protocol adapters, core helpers, and driver specifications for T3 Code.
//!
//! Exports four cohesive layers:
//! - [`acp`]: JSON-RPC 2.0 wire envelopes, stdio NDJSON framing via
//!   [`t3_protocol::provider_env::NdjsonLineCodec`], session updates, permission
//!   evaluation, and tool-call presentation normalization.
//! - [`core`]: deterministic ID allocation, context handoff budgeting and
//!   head/tail UTF-16 truncation, attachment formatting, model catalog
//!   normalization, structured text-generation prompts, failure redaction, and
//!   usage-limit reconciliation.
//! - [`drivers`]: sanitized-env (`BUG-12`) driver launch/probe builders for
//!   `antigravity` (`BUG-11`), `grok` (`BUG-13`/`BUG-14`), all built-in CLIs,
//!   and deterministic `dev_replay` streams.
//! - [`non_acp`]: native wire codecs, SSE parser, and HTTP request builders for
//!   `codex`, `claude`, `cursor`, `pi`, `opencode`, and xAI `grok` REST/SSE.
//!
//! # Examples
//!
//! ```
//! use t3_protocol::provider_env::SanitizedEnv;
//! use t3_providers::drivers::{AntigravityAuthConfig, AntigravityProbeOutcome, probe_antigravity};
//!
//! let env = SanitizedEnv::from_entries([("PATH", "/usr/bin"), ("T3CODE_SECRET", "skip")], false);
//! assert!(env.get("T3CODE_SECRET").is_none());
//!
//! let outcome = probe_antigravity(
//!     None,
//!     &AntigravityAuthConfig::default(),
//!     "antigravity",
//!     "/var/t3",
//!     "/tmp",
//! );
//! assert!(matches!(outcome, AntigravityProbeOutcome::NotInstalled { .. }));
//! ```

pub mod acp;
pub mod core;
pub mod drivers;
pub mod non_acp;
