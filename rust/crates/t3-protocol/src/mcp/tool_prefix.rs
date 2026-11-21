//! Branded MCP tool name and HTTP header matching routed through [`t3_contracts::brand`].

use t3_contracts::brand::{
    ACTIVE_BRAND, BrandConfig, LEGION_CODE, T3_COMPAT, canonical_mcp_tool_suffix,
};

/// Canonicalizes a potentially branded/namespaced MCP tool name to its bare action suffix.
///
/// Accepts `t3_<name>`, `legion_<name>`, `mcp__t3_code__<name>`, `mcp__t3_code__t3_<name>`,
/// `mcp__legion_code__<name>`, `mcp__legion_code__legion_<name>`, and hyphenated variants.
#[must_use]
pub fn canonical_mcp_tool_name(tool_name: &str) -> &str {
    canonical_mcp_tool_suffix(tool_name)
}

/// Returns `true` when `tool_name` carries a recognized T3 or Legion MCP prefix/namespace.
#[must_use]
pub fn has_branded_mcp_tool_prefix(tool_name: &str) -> bool {
    canonical_mcp_tool_suffix(tool_name) != tool_name
}

/// Returns `true` when `tool_name` represents the branded MCP tool `expected_suffix` under
/// either T3 or Legion naming conventions (excluding bare unprefixed tool names unless
/// `expected_suffix` already includes a prefix).
#[must_use]
pub fn is_branded_mcp_tool(tool_name: &str, expected_suffix: &str) -> bool {
    let bare = canonical_mcp_tool_suffix(tool_name);
    if bare == expected_suffix && bare != tool_name {
        return true;
    }
    ACTIVE_BRAND.canonical_mcp_tool_suffix(tool_name) == expected_suffix
        && ACTIVE_BRAND.canonical_mcp_tool_suffix(tool_name) != tool_name
        || T3_COMPAT.canonical_mcp_tool_suffix(tool_name) == expected_suffix
            && T3_COMPAT.canonical_mcp_tool_suffix(tool_name) != tool_name
        || LEGION_CODE.canonical_mcp_tool_suffix(tool_name) == expected_suffix
            && LEGION_CODE.canonical_mcp_tool_suffix(tool_name) != tool_name
}

/// Returns ordered MCP tool name candidates for `suffix` under `brand`, including bare and
/// namespaced forms (`<prefix><suffix>`, `<ns><prefix><suffix>`, `<ns><suffix>`).
#[must_use]
pub fn mcp_tool_candidates_for(brand: &BrandConfig, suffix: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut push_unique = |candidate: String| {
        if !out.contains(&candidate) {
            out.push(candidate);
        }
    };
    push_unique(format!("{}{suffix}", brand.mcp_tool_prefix));
    push_unique(format!(
        "{}{}{suffix}",
        brand.mcp_namespace, brand.mcp_tool_prefix
    ));
    push_unique(format!("{}{suffix}", brand.mcp_namespace));
    push_unique(format!("{}{suffix}", brand.fallback_mcp_tool_prefix));
    push_unique(format!(
        "{}{}{suffix}",
        brand.fallback_mcp_namespace, brand.fallback_mcp_tool_prefix
    ));
    push_unique(format!("{}{suffix}", brand.fallback_mcp_namespace));
    out
}

/// Returns deduplicated MCP tool aliases for `suffix` across [`ACTIVE_BRAND`], [`T3_COMPAT`],
/// and [`LEGION_CODE`].
#[must_use]
pub fn mcp_tool_candidates(suffix: &str) -> Vec<String> {
    let mut out = Vec::new();
    for brand in [ACTIVE_BRAND, T3_COMPAT, LEGION_CODE] {
        for candidate in mcp_tool_candidates_for(&brand, suffix) {
            if !out.contains(&candidate) {
                out.push(candidate);
            }
        }
    }
    out
}

/// Returns deduplicated lowercase HTTP header aliases (`x-t3-<suffix>`, `t3-<suffix>`,
/// `x-legion-<suffix>`) for MCP transport headers.
#[must_use]
pub fn mcp_header_candidates(suffix: &str) -> Vec<String> {
    let mut out = Vec::new();
    for brand in [ACTIVE_BRAND, T3_COMPAT, LEGION_CODE] {
        for candidate in brand.tunnel_header_candidates(suffix) {
            let lower = candidate.to_ascii_lowercase();
            if !out.contains(&lower) {
                out.push(lower);
            }
        }
    }
    out
}

/// Returns `true` when `header_name` matches any T3/Legion alias for `suffix` (ASCII
/// case-insensitive).
#[must_use]
pub fn matches_mcp_header(header_name: &str, suffix: &str) -> bool {
    let normalized = header_name.trim().to_ascii_lowercase();
    mcp_header_candidates(suffix).contains(&normalized)
}
