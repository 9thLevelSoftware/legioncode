//! Sanitized provider/terminal environments, 128 MiB LF-only NDJSON framing,
//! 15-tier CLI maintenance precedence, and terminal session resumption keys.
//!
//! Fixes BUG-08, BUG-12, BUG-13, and BUG-14.
//!
//! # Examples
//!
//! ```
//! use t3_protocol::provider_env::{
//!     MaintenanceInstaller, MaintenanceResolutionInput, MaintenanceTier, NdjsonLineCodec,
//!     ProviderMaintenanceDefinition, SanitizedEnv, TerminalSessionKey, TerminalSessionKeys,
//!     make_targeted_update_action, resolve_provider_maintenance,
//! };
//!
//! // BUG-12: SanitizedEnv blocks T3CODE_* / VITE_* / AppImage / terminal session vars.
//! let env = SanitizedEnv::from_entries(
//!     [
//!         ("PATH", "/tmp/appdir/usr/bin:/usr/bin"),
//!         ("APPDIR", "/tmp/appdir"),
//!         ("T3CODE_HOME", "/secret/home"),
//!         ("PORT", "3773"),
//!     ],
//!     false,
//! );
//! assert_eq!(env.get("T3CODE_HOME"), None);
//! assert_eq!(env.get("PORT"), None);
//! assert_eq!(env.get("PATH"), Some("/usr/bin"));
//!
//! // LF-only NDJSON preserves Unicode U+2028 inside JSON payloads.
//! let mut ndjson = NdjsonLineCodec::default();
//! let frame = "{\"text\":\"a\u{2028}b\"}\r\n";
//! assert_eq!(ndjson.push_bytes(frame.as_bytes())?, vec!["{\"text\":\"a\u{2028}b\"}"]);
//!
//! // BUG-13 & BUG-14: Grok resolves through bun-global, which disables exact pinning.
//! let caps = resolve_provider_maintenance(
//!     &ProviderMaintenanceDefinition::grok(),
//!     &MaintenanceResolutionInput {
//!         resolved_binary_path: Some("/home/u/.bun/bin/grok".to_owned()),
//!         ..Default::default()
//!     },
//! );
//! assert_eq!(caps.installer, MaintenanceInstaller::BunGlobal);
//! assert_eq!(caps.resolved_tier, MaintenanceTier::BunGlobal);
//! assert!(!caps.supports_exact_pin);
//! assert!(make_targeted_update_action(&caps, "1.2.3").is_none());
//!
//! // BUG-08: LRU-evicted terminal keys stay resumable unlike explicitly closed keys.
//! let mut keys = TerminalSessionKeys::with_capacity(1);
//! let k1 = TerminalSessionKey::new("t1", "term-1");
//! let k2 = TerminalSessionKey::new("t1", "term-2");
//! keys.mark_inactive(k1.clone());
//! keys.mark_inactive(k2.clone());
//! assert!(keys.is_evicted(&k1));
//! assert!(keys.can_resume(&k1));
//! keys.close(k2.clone());
//! assert!(!keys.can_resume(&k2));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod env;
mod maintenance;
mod maintenance_js;
mod maintenance_npm;
mod maintenance_types;
mod ndjson;
mod terminal_keys;

pub use env::{
    APPIMAGE_ENV_VARS, SanitizedEnv, TERMINAL_ENV_BLOCKLIST, is_blocked_ambient_env_key,
};
pub use maintenance::{
    make_targeted_update_action, resolve_latest_update_action, resolve_provider_maintenance,
};
pub use maintenance_js::supports_exact_pin;
pub use maintenance_npm::{
    ChannelUpdatePromptOutcome, HOMEBREW_LATEST_CASK_SWITCH_COMMAND,
    HOMEBREW_STABLE_CASK_UPDATE_COMMAND, NpmBatchQueryPlan, SemverVersion,
    channel_latest_version_below, compare_semver, evaluate_channel_update_prompt, filter_npm_env,
    is_ignored_npm_env_var, npm_prefix_single_flight_key, parse_npm_ls_versions,
    parse_npm_outdated_latest_versions, parse_semver, plan_npm_prefix_batch_query,
    provider_update_channel_lags,
};
pub use maintenance_types::{
    MaintenanceCapabilities, MaintenanceInstaller, MaintenanceProbe, MaintenanceResolutionInput,
    MaintenanceShimMode, MaintenanceTier, ProviderMaintenanceDefinition, UpdateCommandSpec,
};
pub use ndjson::{MAX_NDJSON_LINE_BYTES, NdjsonCodecError, NdjsonLineCodec};
pub use terminal_keys::{TerminalResumeStatus, TerminalSessionKey, TerminalSessionKeys};
