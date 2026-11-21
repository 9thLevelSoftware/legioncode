#![forbid(unsafe_code)]
//! Remote access, SSH tunnel management, Tailscale discovery, peer links, secret storage,
//! OTLP trace summaries, and environment theme management for T3 Code.
//!
//! ```
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use t3_remote::theme::is_valid_theme_id;
//! use t3_remote::tunnel::tailscale::build_tailscale_https_base_url;
//!
//! assert!(is_valid_theme_id("nord-night"));
//! assert_eq!(
//!     build_tailscale_https_base_url("devbox.tailnet.ts.net", Some(443))?,
//!     "https://devbox.tailnet.ts.net/",
//! );
//! # Ok(())
//! # }
//! ```

mod host_class;
mod theme_publish;
mod theme_settings;
mod theme_validate;
mod trace_format;
mod trace_parse;

pub mod acp_mcp_bridge;
pub mod live_network;
pub mod peer;
pub mod secrets;
pub mod ssh_helper;
pub mod theme;
pub mod thread_client;
pub mod trace;
pub mod tunnel;
