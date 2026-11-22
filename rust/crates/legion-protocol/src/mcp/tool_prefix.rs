//! Branded MCP tool name and HTTP header matching routed through [`legion_contracts::brand`].

use legion_contracts::brand::{ACTIVE_BRAND, BrandConfig, canonical_mcp_tool_suffix};

/// Canonicalizes a potentially branded/namespaced MCP tool name to its bare action suffix.
///
/// Accepts `legion_<name>`, `mcp__legion_code__<name>`, `mcp__legion_code__legion_<name>`,
/// and hyphenated namespace variants.
#[must_use]
pub fn canonical_mcp_tool_name(tool_name: &str) -> &str {
    canonical_mcp_tool_suffix(tool_name)
}

/// Returns `true` when `tool_name` carries a recognized `LegionCode` MCP prefix/namespace.
#[must_use]
pub fn has_branded_mcp_tool_prefix(tool_name: &str) -> bool {
    canonical_mcp_tool_suffix(tool_name) != tool_name
}

/// Returns `true` when `tool_name` represents the branded MCP tool `expected_suffix` under
/// `LegionCode` naming conventions (excluding bare unprefixed tool names unless
/// `expected_suffix` already includes a prefix).
#[must_use]
pub fn is_branded_mcp_tool(tool_name: &str, expected_suffix: &str) -> bool {
    let bare = canonical_mcp_tool_suffix(tool_name);
    if bare == expected_suffix && bare != tool_name {
        return true;
    }
    ACTIVE_BRAND.canonical_mcp_tool_suffix(tool_name) == expected_suffix
        && ACTIVE_BRAND.canonical_mcp_tool_suffix(tool_name) != tool_name
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
    out
}

/// Returns deduplicated MCP tool aliases for `suffix` under [`ACTIVE_BRAND`].
#[must_use]
pub fn mcp_tool_candidates(suffix: &str) -> Vec<String> {
    mcp_tool_candidates_for(&ACTIVE_BRAND, suffix)
}

/// Returns deduplicated lowercase HTTP header aliases (`x-legioncode-<suffix>`) for MCP
/// transport headers.
#[must_use]
pub fn mcp_header_candidates(suffix: &str) -> Vec<String> {
    ACTIVE_BRAND
        .tunnel_header_candidates(suffix)
        .into_iter()
        .map(|candidate| candidate.to_ascii_lowercase())
        .collect()
}

/// Returns `true` when `header_name` matches the `LegionCode` header alias for `suffix` (ASCII
/// case-insensitive).
#[must_use]
pub fn matches_mcp_header(header_name: &str, suffix: &str) -> bool {
    let normalized = header_name.trim().to_ascii_lowercase();
    mcp_header_candidates(suffix).contains(&normalized)
}
