//! Desktop custom-scheme asset router, CSP header generator, and display-clamped
//! `WindowBounds` sanitizer (`apps/desktop/src/electron/ElectronProtocol.ts`,
//! `apps/desktop/src/window/DesktopWindow.ts`, `apps/desktop/src/settings/DesktopAppSettings.ts`).
//!
//! Pure Safe Rust and `wasm32-unknown-unknown` clean (filesystem probing is injected by callers).

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

pub use crate::desktop_bounds::{
    DEFAULT_MAIN_WINDOW_HEIGHT, DEFAULT_MAIN_WINDOW_WIDTH, DisplayRect, InitialWindowBounds,
    MIN_MAIN_WINDOW_HEIGHT, MIN_MAIN_WINDOW_WIDTH, SAVED_BOUNDS_EDGE_TOLERANCE_PX, WindowBounds,
};
pub use crate::desktop_scheme::{build_desktop_csp, mime_type_for_path};
use crate::desktop_scheme::{
    has_file_extension, normalize_safe_relative_path, percent_decode_strict, rejected,
    sanitize_hostname_token, split_custom_scheme_url,
};

/// Canonical desktop custom-scheme host (`ElectronProtocol.ts:15`).
pub const DESKTOP_HOST: &str = "app";
/// Production desktop protocol scheme (`ElectronProtocol.ts:16`).
pub const DESKTOP_PRODUCTION_SCHEME: &str = "t3code";
/// Development desktop protocol scheme (`ElectronProtocol.ts:17`).
pub const DESKTOP_DEVELOPMENT_SCHEME: &str = "t3code-dev";

/// Reason a custom-scheme request was rejected by [`DesktopSchemeRouter`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesktopRejectionReason {
    /// Request scheme/host/authority did not match `<scheme>://app`.
    ForeignOrigin,
    /// HTTP method was not `GET` or `HEAD` (`405`).
    MethodNotAllowed,
    /// Malformed URL, invalid percent-encoding, or NUL byte (`400`).
    BadEncoding,
    /// Path attempted directory traversal (`..` / `.` / `\`) (`404`).
    PathTraversal,
    /// Asset did not exist and `Accept` did not allow SPA fallback (`404`).
    NotFound,
}

/// Resolution produced by [`DesktopSchemeRouter::route`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DesktopRouteDecision {
    /// Serve a concrete bundled asset relative to the asset root (`200`).
    ServeAsset {
        /// Normalized relative path (e.g. `"app.js"`).
        relative_path: String,
        /// Resolved MIME `Content-Type` header value.
        mime_type: &'static str,
        /// Generated `Content-Security-Policy` header value.
        content_security_policy: String,
        /// `true` when method is `HEAD` and body should be omitted.
        head_only: bool,
    },
    /// Serve the SPA fallback `"index.html"` (`200`).
    FallbackSpaIndex {
        /// Always `"index.html"`.
        relative_path: &'static str,
        /// `"text/html; charset=utf-8"`.
        mime_type: &'static str,
        /// Generated `Content-Security-Policy` header value.
        content_security_policy: String,
        /// `true` when method is `HEAD`.
        head_only: bool,
    },
    /// Request was rejected with HTTP `status` and CSP header attached.
    Rejected {
        /// HTTP status (`400`, `404`, or `405`).
        status: u16,
        /// Machine-readable rejection reason.
        reason: DesktopRejectionReason,
        /// Generated `Content-Security-Policy` header value.
        content_security_policy: String,
    },
}

impl DesktopRouteDecision {
    /// Returns the HTTP status code (`200`, `400`, `404`, or `405`).
    pub const fn status(&self) -> u16 {
        match self {
            Self::ServeAsset { .. } | Self::FallbackSpaIndex { .. } => 200,
            Self::Rejected { status, .. } => *status,
        }
    }
}

/// Custom-scheme asset router and CSP builder for `t3code://app/*` (`t3code-dev://app/*`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesktopSchemeRouter {
    scheme: String,
    clerk_frontend_api_hostname: Option<String>,
    assets: BTreeSet<String>,
}

impl DesktopSchemeRouter {
    /// Creates a router for `scheme` (e.g. `"t3code"` or `"t3code-dev"`).
    pub fn new(scheme: impl Into<String>, clerk_frontend_api_hostname: Option<&str>) -> Self {
        Self {
            scheme: scheme.into(),
            clerk_frontend_api_hostname: clerk_frontend_api_hostname
                .and_then(sanitize_hostname_token),
            assets: BTreeSet::new(),
        }
    }

    /// Registers known bundled relative asset paths (e.g. `["index.html", "assets/app.js"]`).
    #[must_use]
    pub fn with_assets<I, S>(mut self, paths: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        for p in paths {
            let trimmed = p.as_ref().trim_start_matches('/');
            if !trimmed.is_empty() {
                self.assets.insert(trimmed.to_owned());
            }
        }
        self
    }

    /// Configured custom protocol scheme (`"t3code"` or `"t3code-dev"`).
    pub fn scheme(&self) -> &str {
        &self.scheme
    }

    /// Builds the `Content-Security-Policy` header matching `ElectronProtocol.ts:69-98`.
    pub fn content_security_policy(&self) -> String {
        build_desktop_csp(&self.scheme, self.clerk_frontend_api_hostname.as_deref())
    }

    /// Routes a custom-scheme request against the registered `assets` set.
    pub fn route(
        &self,
        method: &str,
        url: &str,
        accept_header: Option<&str>,
    ) -> DesktopRouteDecision {
        self.route_with(method, url, accept_header, |rel| self.assets.contains(rel))
    }

    /// Routes a custom-scheme request using a custom `has_asset` lookup callback.
    pub fn route_with<F>(
        &self,
        method: &str,
        url: &str,
        accept_header: Option<&str>,
        has_asset: F,
    ) -> DesktopRouteDecision
    where
        F: Fn(&str) -> bool,
    {
        let csp = self.content_security_policy();
        let Some((scheme, authority, raw_path)) = split_custom_scheme_url(url) else {
            return rejected(400, DesktopRejectionReason::BadEncoding, csp);
        };

        if !scheme.eq_ignore_ascii_case(&self.scheme)
            || !authority.eq_ignore_ascii_case(DESKTOP_HOST)
        {
            return rejected(404, DesktopRejectionReason::ForeignOrigin, csp);
        }

        let head_only = if method.eq_ignore_ascii_case("GET") {
            false
        } else if method.eq_ignore_ascii_case("HEAD") {
            true
        } else {
            return rejected(405, DesktopRejectionReason::MethodNotAllowed, csp);
        };

        let Some(decoded_path) = percent_decode_strict(raw_path) else {
            return rejected(400, DesktopRejectionReason::BadEncoding, csp);
        };

        if decoded_path.contains('\0') {
            return rejected(400, DesktopRejectionReason::BadEncoding, csp);
        }

        let Some(relative) = normalize_safe_relative_path(&decoded_path) else {
            return rejected(404, DesktopRejectionReason::PathTraversal, csp);
        };

        if !relative.is_empty() && has_asset(&relative) {
            return DesktopRouteDecision::ServeAsset {
                mime_type: mime_type_for_path(&relative),
                relative_path: relative,
                content_security_policy: csp,
                head_only,
            };
        }

        let wants_html = accept_header.is_some_and(|h| h.contains("text/html"));
        if !relative.is_empty() && has_file_extension(&relative) && !wants_html {
            return rejected(404, DesktopRejectionReason::NotFound, csp);
        }

        if has_asset("index.html") || self.assets.is_empty() {
            return DesktopRouteDecision::FallbackSpaIndex {
                relative_path: "index.html",
                mime_type: "text/html; charset=utf-8",
                content_security_policy: csp,
                head_only,
            };
        }

        rejected(404, DesktopRejectionReason::NotFound, csp)
    }
}
