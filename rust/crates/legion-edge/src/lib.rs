#![forbid(unsafe_code)]
//! Cloudflare Workers relay, hook inbox, task-cache, desktop-update, and locale parity logic
//! for `LegionCode`.
//!
//! Every module is pure Safe Rust with explicit timestamps (`now_ms: u64` / `now_secs: u64`) and
//! zero blocking OS or async runtime dependencies, allowing identical execution on native hosts
//! and `wasm32-unknown-unknown`.
//!
//! # Examples
//!
//! ```
//! use legion_edge::desktop_updates::{DesktopUpdateChannel, DesktopUpdateMachine, PendingUpdate, VerifiedArtifact, render_aur_launcher};
//! use legion_edge::i18n::{SUPPORTED_LOCALES, resolve_locale};
//! use legion_edge::relay_hooks::{CloudflareApiPauseGate, DeliveryOutcome, HeldHook, HookInboxState};
//! use legion_edge::task_cache::{MAX_MULTIPART_PARTS, validate_part_number};
//!
//! // BUG-29: wake increments generation so stale failed sends never corrupt new target state.
//! let mut inbox = HookInboxState::new();
//! let hook = HeldHook::new("del-1", "hook-1", 1_000, "POST", b"{}".to_vec());
//! assert!(inbox.hold(hook, "https://old.tunnel.test", 1_000).is_ok());
//! let batch = inbox.begin_delivery_batch(1_000).ok_or("missing batch")?;
//! assert!(inbox.wake("https://new.tunnel.test"));
//! let outcome = inbox.record_delivery_outcome(
//!     batch.target_generation,
//!     "del-1",
//!     DeliveryOutcome::Unreachable,
//!     8_500,
//! );
//! assert!(outcome.is_ignored_stale());
//! assert_eq!(inbox.target().map(|t| t.failures), Some(0));
//!
//! // BUG-32: transient pause read failures do not advance the 5s refresh window.
//! let mut gate = CloudflareApiPauseGate::new();
//! assert!(gate.apply_shared_pause_read::<&str>(1_000, Err("d1 timeout")).is_err());
//! assert!(gate.should_refresh(1_001));
//!
//! // BUG-30: multipart part numbers reject 0 and > 64 at PUT.
//! assert_eq!(validate_part_number(MAX_MULTIPART_PARTS), Ok(64));
//! assert!(validate_part_number(65).is_err());
//!
//! // BUG-28 & BUG-31: failed V2 check/download keeps V1 retained; AUR nightly has distinct ID.
//! let mut updater = DesktopUpdateMachine::new(DesktopUpdateChannel::Latest, "1.0.0");
//! updater.on_download_complete(VerifiedArtifact::new("1.1.0", "digest-v1", 1024));
//! updater.on_update_available(PendingUpdate::new("1.2.0", "2026-11-20T00:00:00Z"));
//! updater.on_download_failure("network timeout");
//! assert_eq!(updater.installable_artifact().map(|a| a.version.as_str()), Some("1.1.0"));
//! let nightly = render_aur_launcher(DesktopUpdateChannel::Nightly);
//! assert_eq!(nightly.desktop_entry_id, "dev.legioncode.LegionCodeNightly.desktop");
//! assert!(nightly.launcher_script.contains("LEGIONCODE_DESKTOP_LAUNCHER_PATH=/usr/bin/legioncode-nightly"));
//!
//! // BUG-27: canonical 10-locale registry restores Swedish parity across all surfaces.
//! assert_eq!(SUPPORTED_LOCALES.len(), 10);
//! assert_eq!(resolve_locale("system", &["sv-SE", "en-US"]), "sv");
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod boot_service_units_parse;
mod boot_service_units_protection;
mod boot_service_units_render;
mod cf_bindings_cron;
mod cf_bindings_d1;
mod cf_bindings_r2;
mod cf_bindings_relay;
mod desktop_updates_aur;
mod desktop_updates_resume;
mod desktop_updates_types;
mod relay_hooks_pause;
mod relay_hooks_types;
mod task_cache_admission;
mod task_cache_ledger;
mod task_cache_ledger_types;
mod task_cache_savings;

pub mod boot_service_units;
pub mod cf_bindings;
pub mod desktop_updates;
pub mod i18n;
pub mod notification_text;
pub mod relay_hooks;
pub mod task_cache;
