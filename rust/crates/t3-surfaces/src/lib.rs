#![forbid(unsafe_code)]
//! Shared `wasm32`-clean surface shells for T3 Code mobile, desktop, and Cloudflare Workers.
//!
//! Modules close `BUG-22`, `BUG-23`, `BUG-24`, and `BUG-26` while keeping zero `unsafe` blocks,
//! zero `std::fs` / `std::net` / `tokio` usage, and full `wasm32-unknown-unknown` compatibility:
//! - [`mobile_diff`] (`BUG-22`): `FontStyleFlags`, `AttributedDiffRun`, and `decorate_diff_runs`.
//! - [`mobile_composer`] (`BUG-23`): `ComposerClipboardContext` normalizing missing/whitespace IDs.
//! - [`mobile_i18n`] (`BUG-24`): localized `MobileNavigationPresenter` & `AgentLiveActivityPresenter`.
//! - [`mobile_tunnel`] (`BUG-26`): `TerminalThemeFileStore`, `HttpHeaderHostSwap`, and
//!   `ScopedWebViewTunnelRegistry`.
//! - [`desktop`]: `DesktopSchemeRouter` (`t3code://app/*`, CSP, MIME) and display-clamped
//!   `WindowBounds`.
//! - [`cf_worker`]: `CloudflareWorkerRouter` wiring [`t3_edge`] hooks, pause gate, and task cache.
//!
//! # Examples
//!
//! ```
//! use t3_client_core::diff_markdown::Utf16Span;
//! use t3_surfaces::desktop::{DesktopSchemeRouter, DisplayRect, WindowBounds};
//! use t3_surfaces::mobile_composer::ComposerClipboardContext;
//! use t3_surfaces::mobile_diff::{AttributedDiffRun, FontStyleFlags, decorate_diff_runs};
//! use t3_surfaces::mobile_i18n::{MobileNavigationPresenter, NavigationTabKind};
//! use t3_surfaces::mobile_tunnel::{HttpHeaderHostSwap, HttpProxyPhase, TerminalThemeFileStore};
//!
//! // BUG-22: word-diff highlight splitting preserves BOLD | ITALIC and non-color attributes.
//! let run = AttributedDiffRun::new(0, 6, "foobar")
//!     .with_font_style(FontStyleFlags::BOLD | FontStyleFlags::ITALIC)
//!     .with_attribute("scope", "identifier");
//! let decorated = decorate_diff_runs(&[run], &[Utf16Span { start: 3, end: 6 }]);
//! assert_eq!(decorated.len(), 2);
//! assert!(decorated[1].word_highlighted);
//! assert_eq!(decorated[1].font_style, FontStyleFlags::BOLD | FontStyleFlags::ITALIC);
//! assert_eq!(decorated[1].attributes.get("scope").map(String::as_str), Some("identifier"));
//!
//! // BUG-23: whitespace or empty context IDs normalize to None and never emit "/)".
//! let ctx = ComposerClipboardContext::new(Some("   "), None);
//! assert_eq!(ctx.context_id(), None);
//! assert_eq!(ctx.chip_pattern(), None);
//!
//! // BUG-24: mobile navigation strings resolve via t3_edge::i18n without English fallback leaks.
//! let nav = MobileNavigationPresenter::for_locale("de");
//! assert_eq!(nav.tab_title(NavigationTabKind::Settings), "Einstellungen");
//!
//! // BUG-26: per-pane filenames & header-only Host swap.
//! let mut store = TerminalThemeFileStore::new();
//! let p1 = store.upsert("pane/1", "font-size = 14\n");
//! assert!(p1.filename.starts_with("t3code-ghostty-pane-1-"));
//! let mut swap = HttpHeaderHostSwap::new("t3lo.test", "localhost");
//! let out = swap.push(b"POST / HTTP/1.1\r\nHost: t3lo.test\r\n\r\n{\"h\":\"Host: t3lo.test\"}");
//! assert_eq!(swap.phase(), HttpProxyPhase::BodyPassthrough);
//! assert!(String::from_utf8_lossy(&out).contains("{\"h\":\"Host: t3lo.test\"}"));
//!
//! // Desktop custom scheme routing & clamped WindowBounds.
//! let router = DesktopSchemeRouter::new("t3code", None).with_assets(["index.html", "app.js"]);
//! assert_eq!(router.route("GET", "t3code://app/settings", None).status(), 200);
//! let clamped = WindowBounds { x: 5000, y: 5000, width: 800, height: 600 }
//!     .sanitize_clamped(&[DisplayRect { x: 0, y: 0, width: 1920, height: 1080 }]);
//! assert_eq!(clamped.x, 1120);
//! ```

mod cf_worker_cache;
mod cf_worker_hooks;
mod desktop_bounds;
mod mobile_diff_types;
mod mobile_i18n_activity;
mod mobile_i18n_activity_catalog;
mod mobile_i18n_activity_catalog_intl;
mod mobile_i18n_activity_types;
mod mobile_i18n_nav_catalog;
mod mobile_tunnel_proxy;
mod mobile_tunnel_theme;

pub mod cf_worker;
pub mod desktop;
pub mod mobile_composer;
pub mod mobile_diff;
pub mod mobile_i18n;
pub mod mobile_tunnel;

/// Protocol version shared across web, desktop, and mobile client surfaces.
///
/// # Examples
///
/// ```
/// assert_eq!(t3_surfaces::SURFACE_PROTOCOL_VERSION, 1);
/// ```
pub const SURFACE_PROTOCOL_VERSION: u32 = 1;

#[cfg(test)]
mod tests {
    use super::SURFACE_PROTOCOL_VERSION;

    #[test]
    fn surface_protocol_version_is_one() {
        assert_eq!(SURFACE_PROTOCOL_VERSION, 1);
    }
}
