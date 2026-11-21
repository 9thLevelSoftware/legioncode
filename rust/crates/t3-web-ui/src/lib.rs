//! Pure Safe Rust Model-View-Update Web UI engine for T3 Code (`t3-web-ui`).
//!
//! Zero OS/async/wasm-bindgen dependencies so every view, reducer, diff inspector,
//! command palette, locale switcher, and terminal panel runs natively in unit tests
//! and compiles cleanly for `wasm32-unknown-unknown`.
//!
//! # Examples
//!
//! ```
//! use t3_web_ui::{RightPaneTab, WebAppAction, WebAppEngine, emit_web_app_assets, escape_html};
//!
//! let mut engine = WebAppEngine::new("sv-SE", 1_000);
//! assert_eq!(engine.state().locale.catalog_locale, "sv");
//! let _ = engine.dispatch(WebAppAction::SetRightTab {
//!     tab: RightPaneTab::Terminal,
//! });
//! assert!(engine.render_html().contains(r#"data-testid="terminal-grid""#));
//! assert_eq!(escape_html("<script>"), "&lt;script&gt;");
//! assert!(emit_web_app_assets().app_mjs.contains("WasmWebApp"));
//! ```

#![forbid(unsafe_code)]

pub mod actions;
pub mod actions_features;
pub mod actions_types;
pub mod bundle;
pub mod effects;
pub mod hdl_highlight;
mod hdl_keywords;
mod i18n_web;
mod i18n_web_features;
mod i18n_web_intl;
pub mod state;
pub mod state_features;
pub mod state_snapshot;
pub mod state_types;
pub mod update;
mod update_dispatch;
mod update_dispatch_features;
mod update_features;
mod update_fleet;
mod update_palette;
mod update_panels;
mod update_projection;
mod update_settings_features;
mod update_stream;
mod update_timeline;
mod update_workspace;
pub mod view;
mod view_chat;
mod view_features;
mod view_palette;
mod view_panels;

pub use actions::{RightPaneTab, WebAppAction};
pub use bundle::{WebAppStaticAssets, emit_web_app_assets};
pub use effects::{WebHostEffect, WebUiError};
pub use state::{ConnectionStatus, WebAppState, WebAppStateSnapshot};
pub use update::WebAppEngine;
pub use view::{escape_html, render_workspace_html};

/// Returns the crate version string for `t3-web-ui`.
pub const fn web_ui_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
