//! Antigravity ACP usage math, model-id/rate-model mapping, catalog labeling, and SQLite DB filtering.

use std::sync::OnceLock;

use regex::Regex;

use crate::core::ServerModelEntry;

/// Normalized per-turn prompt usage reported by an ACP adapter (`602e5b32f5`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcpTurnTokenUsage {
    /// Total input tokens processed (includes cached reads for Gemini/Antigravity).
    pub input_tokens: u64,
    /// Total output tokens emitted (already includes thought tokens on Antigravity 1.5+).
    pub output_tokens: u64,
    /// Prompt tokens read from cache.
    pub cache_read_tokens: u64,
    /// Prompt tokens written to cache, clamped to `input_tokens - cache_read_tokens`.
    pub cache_creation_tokens: u64,
    /// Reasoning/thought tokens clamped to `output_tokens`.
    pub reasoning_tokens: u64,
}

/// Raw ACP prompt usage counters before provider normalization.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RawAcpUsageCounts {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_read_tokens: u64,
    pub cached_write_tokens: u64,
    pub thought_tokens: u64,
}

/// Normalizes raw ACP prompt usage counters for persistence (`602e5b32f5`).
///
/// For Antigravity / Gemini 1.5+ (`gemini_style == true`), `input_tokens` excludes
/// `cached_read_tokens` so we add them back, while `output_tokens` ALREADY includes
/// `thought_tokens` so `output_tokens` is never double-counted.
#[must_use]
pub fn acp_per_turn_prompt_usage(gemini_style: bool, raw: RawAcpUsageCounts) -> AcpTurnTokenUsage {
    let input_tokens = if gemini_style {
        raw.input_tokens.saturating_add(raw.cached_read_tokens)
    } else {
        raw.input_tokens
    };
    let output_tokens = raw.output_tokens;
    let cache_read_tokens = raw.cached_read_tokens.min(input_tokens);
    let cache_creation_tokens = raw
        .cached_write_tokens
        .min(input_tokens.saturating_sub(cache_read_tokens));
    let reasoning_tokens = raw.thought_tokens.min(output_tokens);
    AcpTurnTokenUsage {
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_creation_tokens,
        reasoning_tokens,
    }
}

/// Maps Antigravity SQLite numeric model IDs to canonical model slugs (`755de001f2`).
#[must_use]
pub const fn antigravity_model_id_lookup(id: u32) -> Option<&'static str> {
    match id {
        1417..=1419 => Some("gemini-4-argon"),
        1537 => Some("gemini-pro-orchestrator-neon"),
        _ => None,
    }
}

fn rate_suffix_regex() -> Option<&'static Regex> {
    static RE: OnceLock<Option<Regex>> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"-(?:extra-low|low|medium|high|tiered|control|thinking|agent|orchestrator)$")
            .ok()
    })
    .as_ref()
}

fn gemini_3_flash_agent_regex() -> Option<&'static Regex> {
    static RE: OnceLock<Option<Regex>> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^gemini-3-flash-(?:agent(?:-[ab])?|[ab])$").ok())
        .as_ref()
}

/// Maps an Antigravity routed/effort variant slug to its base billing rate model (`755de001f2`).
#[must_use]
pub fn antigravity_rate_model(model: &str) -> String {
    if gemini_3_flash_agent_regex().is_some_and(|re| re.is_match(model)) {
        return "gemini-3.5-flash".to_owned();
    }
    match model {
        "gemini-3.7-flash-safety-le" => return "gemini-3.7-flash".to_owned(),
        "gemini-pro-orchestrator-neon" => return "gemini-4-neon".to_owned(),
        "gemini-pro-orchestrator-g1-pro" => return "gemini-4-argon".to_owned(),
        _ => {}
    }
    let base = rate_suffix_regex()
        .map_or_else(|| model.to_owned(), |re| re.replace(model, "").into_owned());
    if base == "gemini-3-flash" {
        "gemini-3-flash-preview".to_owned()
    } else {
        base
    }
}

const REGISTRY_LABEL_DRIVERS: &[&str] = &["cursor", "grok", "antigravity"];

/// Driver catalog snapshot passed into [`catalog_model_label`].
#[derive(Debug, Clone)]
pub struct ProviderCatalogModels<'a> {
    /// Driver slug (`"cursor"`, `"grok"`, `"antigravity"`, ...).
    pub driver: &'a str,
    /// Non-custom and custom models advertised by the provider instance.
    pub models: &'a [(ServerModelEntry, &'a [&'a str])],
}

/// Resolves the canonical catalog slug matching any candidate identifier on a
/// `cursor`, `grok`, or `antigravity` non-custom entry by `slug` or `aliases` (`46b626263f`).
#[must_use]
pub fn catalog_model_label(
    providers: &[ProviderCatalogModels<'_>],
    candidates: &[&str],
) -> Option<String> {
    for provider in providers {
        if !REGISTRY_LABEL_DRIVERS.contains(&provider.driver) {
            continue;
        }
        for (entry, aliases) in provider.models {
            if entry.is_custom {
                continue;
            }
            if candidates.contains(&entry.slug.as_str())
                || aliases.iter().any(|alias| candidates.contains(alias))
            {
                return Some(entry.slug.clone());
            }
        }
    }
    None
}

/// Returns `true` when the inspected SQLite `sqlite_master` table set belongs to an
/// Antigravity conversation trajectory store (`gen_metadata` or `steps`), skipping
/// auxiliary index DBs such as `conversation_summaries.db` (`490a0d687b`).
#[must_use]
pub fn is_antigravity_conversation_db_tables<'a, I>(table_names: I) -> bool
where
    I: IntoIterator<Item = &'a str>,
{
    table_names
        .into_iter()
        .any(|name| name == "gen_metadata" || name == "steps")
}
