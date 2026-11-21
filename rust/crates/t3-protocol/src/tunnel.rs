//! Preview-tunnel HTTP status header constants and [`t3_contracts::brand`] alias helpers (`#39415`).

use t3_contracts::brand::{ACTIVE_BRAND, BrandConfig, LEGION_CODE, T3_COMPAT};

/// Canonical HTTP header suffix indicating the local preview target refused connection.
pub const PREVIEW_TUNNEL_PORT_REFUSED_SUFFIX: &str = "tunnel-port-refused";

/// Canonical HTTP header suffix indicating the preview tunnel proxy is disabled.
pub const PREVIEW_TUNNEL_DISABLED_SUFFIX: &str = "tunnel-disabled";

/// Canonical lowercase preview-tunnel port-refused header (`x-t3-tunnel-port-refused`).
pub const PREVIEW_TUNNEL_PORT_REFUSED_HEADER: &str = "x-t3-tunnel-port-refused";

/// Canonical lowercase preview-tunnel disabled header (`x-t3-tunnel-disabled`).
pub const PREVIEW_TUNNEL_DISABLED_HEADER: &str = "x-t3-tunnel-disabled";

/// Reason signaled by preview-tunnel response status headers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PreviewTunnelStatusHeader {
    /// Target local preview port refused the TCP connection (`tunnel-port-refused`).
    PortRefused,
    /// Preview tunnel feature is disabled on the host server (`tunnel-disabled`).
    Disabled,
}

impl PreviewTunnelStatusHeader {
    /// Returns the unprefixed header suffix for this status.
    #[must_use]
    pub const fn suffix(self) -> &'static str {
        match self {
            Self::PortRefused => PREVIEW_TUNNEL_PORT_REFUSED_SUFFIX,
            Self::Disabled => PREVIEW_TUNNEL_DISABLED_SUFFIX,
        }
    }

    /// Returns the canonical `x-t3-*` header name for this status.
    #[must_use]
    pub const fn canonical_header(self) -> &'static str {
        match self {
            Self::PortRefused => PREVIEW_TUNNEL_PORT_REFUSED_HEADER,
            Self::Disabled => PREVIEW_TUNNEL_DISABLED_HEADER,
        }
    }
}

/// Returns ordered header names (`[primary, fallback]`) for `status` under `brand`.
#[must_use]
pub fn preview_tunnel_header_candidates(
    brand: &BrandConfig,
    status: PreviewTunnelStatusHeader,
) -> Vec<String> {
    brand.tunnel_header_candidates(status.suffix())
}

/// Returns deduplicated lowercase header aliases recognized for `status` across
/// [`ACTIVE_BRAND`], [`T3_COMPAT`], and [`LEGION_CODE`].
#[must_use]
pub fn all_preview_tunnel_header_aliases(status: PreviewTunnelStatusHeader) -> Vec<String> {
    let mut out = Vec::new();
    for brand in [ACTIVE_BRAND, T3_COMPAT, LEGION_CODE] {
        for candidate in brand.tunnel_header_candidates(status.suffix()) {
            let lower = candidate.to_ascii_lowercase();
            if !out.contains(&lower) {
                out.push(lower);
            }
        }
    }
    out
}

/// Returns `true` when `header_name` matches any brand alias for `status` (ASCII case-insensitive).
#[must_use]
pub fn matches_preview_tunnel_header(header_name: &str, status: PreviewTunnelStatusHeader) -> bool {
    let normalized = header_name.trim().to_ascii_lowercase();
    all_preview_tunnel_header_aliases(status).contains(&normalized)
}

/// Returns `true` when an HTTP status flag header value enables the flag (`"1"` or `"true"`).
#[must_use]
pub fn is_truthy_tunnel_status_value(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed == "1" || trimmed.eq_ignore_ascii_case("true")
}

/// Inspects `(name, value)` HTTP response headers and returns the highest-priority truthy
/// preview-tunnel status signaled (`Disabled` takes precedence over `PortRefused`).
pub fn classify_preview_tunnel_headers<'a, I>(headers: I) -> Option<PreviewTunnelStatusHeader>
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    let mut saw_port_refused = false;
    for (name, value) in headers {
        if !is_truthy_tunnel_status_value(value) {
            continue;
        }
        if matches_preview_tunnel_header(name, PreviewTunnelStatusHeader::Disabled) {
            return Some(PreviewTunnelStatusHeader::Disabled);
        }
        if matches_preview_tunnel_header(name, PreviewTunnelStatusHeader::PortRefused) {
            saw_port_refused = true;
        }
    }
    saw_port_refused.then_some(PreviewTunnelStatusHeader::PortRefused)
}
