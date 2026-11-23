//! Branded MCP tool name and HTTP header matching routed through [`legion_contracts::brand`],
//! with dual `legion_*` / `t3_*` compatibility and tool presentation metadata.

use legion_contracts::brand::{ACTIVE_BRAND, BrandConfig, canonical_mcp_tool_suffix};

const LEGACY_TOOL_PREFIX: &str = "t3_";

/// Primary + legacy compatibility presentation metadata for an MCP tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DualToolPresentation {
    /// Canonical primary wire tool name (`legion_<suffix>`).
    pub primary_name: String,
    /// Legacy alias retained for dual client compatibility (`t3_<suffix>`).
    pub legacy_alias: String,
    /// All recognized wire and provider-namespaced aliases (`legion_*` ordered before `t3_*`).
    pub aliases: Vec<String>,
    /// Human-readable presentation title.
    pub title: String,
    /// Tool summary description shown in `tools/list`.
    pub description: String,
}

/// Strips `<server>.` dotted tool qualifiers only when the remainder starts with a known prefix.
fn strip_dotted_tool_qualifier(tool_name: &str) -> &str {
    if let Some((_, rest)) = tool_name.split_once('.')
        && (rest.starts_with(ACTIVE_BRAND.mcp_tool_prefix) || rest.starts_with(LEGACY_TOOL_PREFIX))
    {
        return rest;
    }
    tool_name
}

/// Canonicalizes a potentially branded/namespaced MCP tool name to its bare action suffix.
///
/// Accepts `legion_<name>`, `t3_<name>`, `mcp__legion_code__<name>`,
/// `mcp__legion_code__legion_<name>`, `mcp__legion_code__t3_<name>`, hyphenated namespace
/// variants, and dotted prefixes like `weather.legion_<name>` / `weather.t3_<name>`.
#[must_use]
pub fn canonical_mcp_tool_name(tool_name: &str) -> &str {
    let undotted = strip_dotted_tool_qualifier(tool_name);
    let after_brand = canonical_mcp_tool_suffix(undotted);
    if let Some(rest) = after_brand.strip_prefix(LEGACY_TOOL_PREFIX) {
        return rest;
    }
    after_brand
}

/// Returns `true` when `tool_name` carries a recognized `LegionCode` (`legion_*`) or legacy
/// (`t3_*`) MCP prefix/namespace.
#[must_use]
pub fn has_branded_mcp_tool_prefix(tool_name: &str) -> bool {
    canonical_mcp_tool_name(tool_name) != tool_name
}

/// Returns `true` when `tool_name` represents the branded MCP tool `expected_suffix` under
/// `LegionCode` (`legion_*`) or dual (`t3_*`) naming conventions (excluding bare unprefixed tool
/// names unless `expected_suffix` already carries a prefix).
#[must_use]
pub fn is_branded_mcp_tool(tool_name: &str, expected_suffix: &str) -> bool {
    let bare = canonical_mcp_tool_name(tool_name);
    if bare == expected_suffix && bare != tool_name {
        return true;
    }
    let brand_bare = ACTIVE_BRAND.canonical_mcp_tool_suffix(tool_name);
    brand_bare == expected_suffix && brand_bare != tool_name
}

/// Returns `true` when `tool_name` matches `expected_suffix` under either `legion_*` or `t3_*`
/// presentation.
#[must_use]
pub fn is_dual_branded_mcp_tool(tool_name: &str, expected_suffix: &str) -> bool {
    is_branded_mcp_tool(tool_name, expected_suffix)
}

/// Normalizes any raw/aliased tool name (`t3_thread_wait`, `mcp__legion_code__t3_thread_wait`,
/// `thread_wait`, `legion_thread_wait`) to its primary `legion_<suffix>` presentation name.
#[must_use]
pub fn present_mcp_tool_name(tool_name: &str) -> String {
    let canonical = canonical_mcp_tool_name(tool_name);
    format!("{}{canonical}", ACTIVE_BRAND.mcp_tool_prefix)
}

/// Builds dual presentation metadata (`legion_<suffix>` primary, `t3_<suffix>` legacy alias)
/// for `suffix_or_name`.
#[must_use]
pub fn present_dual_tool(
    suffix_or_name: &str,
    title: impl Into<String>,
    description: impl Into<String>,
) -> DualToolPresentation {
    let canonical = canonical_mcp_tool_name(suffix_or_name);
    DualToolPresentation {
        primary_name: format!("{}{canonical}", ACTIVE_BRAND.mcp_tool_prefix),
        legacy_alias: format!("{LEGACY_TOOL_PREFIX}{canonical}"),
        aliases: mcp_tool_candidates(canonical),
        title: title.into(),
        description: description.into(),
    }
}

/// Returns ordered MCP tool name candidates for `suffix` under `brand`, putting primary `legion_*`
/// names first and legacy `t3_*` aliases second.
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
    push_unique(format!("{LEGACY_TOOL_PREFIX}{suffix}"));
    push_unique(format!(
        "{}{LEGACY_TOOL_PREFIX}{suffix}",
        brand.mcp_namespace
    ));
    out
}

/// Returns deduplicated MCP tool aliases for `suffix` under [`ACTIVE_BRAND`] plus `t3_*` dual
/// compatibility forms.
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
