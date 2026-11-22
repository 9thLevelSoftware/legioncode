//! Provider model catalog normalization, defaults, and concise short names.
//!
//! Mirrors `packages/provider-core/src/server/ModelCatalog.ts` and
//! `modelShortName.ts`.

use std::sync::OnceLock;

use indexmap::IndexMap;
use regex::Regex;
use serde::{Deserialize, Serialize};

/// Input model entry discovered from a provider binary, config, or fallback table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogModelEntry {
    /// Canonical provider model identifier (for example `"claude-sonnet-4-5"`).
    pub slug: String,
    /// Human-readable display label.
    pub name: String,
    /// Whether the model comes from user custom-model settings.
    #[serde(default)]
    pub is_custom: bool,
    /// Advertised reasoning/thinking capability support.
    #[serde(default)]
    pub supports_reasoning: bool,
}

/// Normalized server model record exposed to UI and clients.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerModelEntry {
    /// Canonical provider model slug.
    pub slug: String,
    /// Display name.
    pub name: String,
    /// Compact UI label produced by [`model_short_name`].
    pub short_name: String,
    /// Whether defined via user custom model settings.
    pub is_custom: bool,
    /// Reasoning capability flag.
    pub supports_reasoning: bool,
}

/// Result of classifying and deduplicating built-in and custom catalog entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassifiedCatalogModels {
    /// Ordered deduplicated model list.
    pub models: Vec<ServerModelEntry>,
    /// Selected default model slug, if at least one valid model exists.
    pub default_slug: Option<String>,
}

/// Coordinated catalog builder and resolver for provider model lists.
#[derive(Debug, Clone, Copy, Default)]
pub struct ModelCatalog;

impl ModelCatalog {
    /// Builds a normalized [`ServerModelEntry`] from a [`CatalogModelEntry`].
    #[must_use]
    pub fn to_server_model(entry: CatalogModelEntry) -> ServerModelEntry {
        let short_name = model_short_name(if entry.name.trim().is_empty() {
            &entry.slug
        } else {
            &entry.name
        });
        let name = if entry.name.trim().is_empty() {
            entry.slug.clone()
        } else {
            entry.name
        };
        ServerModelEntry {
            slug: entry.slug,
            name,
            short_name,
            is_custom: entry.is_custom,
            supports_reasoning: entry.supports_reasoning,
        }
    }

    /// Classifies and merges `builtin_models` with `custom_models`, keeping first
    /// occurrence per slug and promoting `custom_models` after built-ins unless
    /// a built-in shares the same slug (marked `is_custom` if specified custom).
    #[must_use]
    pub fn classify(
        builtin_models: Vec<CatalogModelEntry>,
        custom_models: Vec<CatalogModelEntry>,
    ) -> Vec<ServerModelEntry> {
        let mut by_slug: IndexMap<String, ServerModelEntry> = IndexMap::new();
        let all_entries = builtin_models
            .into_iter()
            .chain(custom_models.into_iter().map(|mut entry| {
                entry.is_custom = true;
                entry
            }));
        for mut entry in all_entries {
            let trimmed = entry.slug.trim().to_owned();
            if trimmed.is_empty() {
                continue;
            }
            entry.slug = trimmed;
            by_slug
                .entry(entry.slug.clone())
                .or_insert_with(|| Self::to_server_model(entry));
        }

        by_slug.into_values().collect()
    }

    /// Resolves the active default model slug against `models`, falling back to
    /// the first catalog entry when `preferred_default` is unset or absent.
    #[must_use]
    pub fn apply_default<'a>(
        models: &'a [ServerModelEntry],
        preferred_default: Option<&str>,
    ) -> Option<&'a str> {
        if let Some(preferred) = preferred_default.map(str::trim).filter(|s| !s.is_empty())
            && let Some(matched) = models.iter().find(|m| m.slug == preferred)
        {
            return Some(matched.slug.as_str());
        }
        models.first().map(|m| m.slug.as_str())
    }

    /// Applies catalog merging, deduplication, and default resolution in one pass.
    #[must_use]
    pub fn apply(
        builtin_models: Vec<CatalogModelEntry>,
        custom_models: Vec<CatalogModelEntry>,
        preferred_default: Option<&str>,
    ) -> ClassifiedCatalogModels {
        let models = Self::classify(builtin_models, custom_models);
        let default_slug = Self::apply_default(&models, preferred_default).map(ToOwned::to_owned);
        ClassifiedCatalogModels {
            models,
            default_slug,
        }
    }
}

fn date_suffix_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let re = Regex::new(r"(?i)(?:-\d{8}|-\d{4}-\d{2}-\d{2}|:(?:free|thinking|latest))+$");
        // INFALLIBLE: static constant regex validated against literal syntax.
        re.expect("valid model catalog date/tag suffix regex")
    })
}

/// Derives a concise UI label from a raw model slug or verbose model display name.
///
/// Strips vendor prefixes (`anthropic/`, `openai/`, `google/`, `xai/`, etc.),
/// trailing release-date suffixes (`-20250514`, `-2025-05-14`), and known redundant
/// family prefixes (`claude-`) while preserving major/minor version numbers.
///
/// # Examples
///
/// ```
/// use legion_providers::core::model_short_name;
///
/// assert_eq!(model_short_name("anthropic/claude-sonnet-4-5-20250929"), "Sonnet 4.5");
/// assert_eq!(model_short_name("openai/gpt-5.4"), "GPT-5.4");
/// assert_eq!(model_short_name("xai/grok-4-fast-reasoning"), "Grok 4 Fast Reasoning");
/// ```
#[must_use]
pub fn model_short_name(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let without_vendor = strip_vendor_prefix(trimmed);
    let without_dates = date_suffix_regex().replace(without_vendor, "");
    let cleaned = without_dates
        .trim_start_matches("claude-")
        .trim_start_matches("Claude ")
        .trim();

    if cleaned.is_empty() {
        return without_vendor.to_owned();
    }

    if let Some(rest) = strip_ascii_prefix_ci(cleaned, "gpt-") {
        return format!("GPT-{rest}");
    }
    let lower = cleaned.to_ascii_lowercase();
    if lower.starts_with("o1") || lower.starts_with("o3") || lower.starts_with("o4") {
        return format_hyphen_words(cleaned, false);
    }

    let tokens: Vec<&str> = cleaned
        .split(['-', '_', ' '])
        .filter(|s| !s.is_empty())
        .collect();
    if tokens.is_empty() {
        return cleaned.to_owned();
    }

    let mut out = Vec::with_capacity(tokens.len());
    let mut i = 0usize;
    while i < tokens.len() {
        let tok = tokens[i];
        // Collapse adjacent single-digit version parts (`4`, `5` -> `4.5`).
        if tok.len() == 1
            && tok.as_bytes().first().is_some_and(u8::is_ascii_digit)
            && let Some(next) = tokens.get(i + 1)
            && next.len() == 1
            && next.as_bytes().first().is_some_and(u8::is_ascii_digit)
        {
            out.push(format!("{tok}.{next}"));
            i += 2;
            continue;
        }
        out.push(format_word(tok));
        i += 1;
    }
    out.join(" ")
}

fn strip_ascii_prefix_ci<'a>(raw: &'a str, ascii_prefix: &str) -> Option<&'a str> {
    let head = raw.get(..ascii_prefix.len())?;
    head.eq_ignore_ascii_case(ascii_prefix)
        .then(|| raw.get(ascii_prefix.len()..).unwrap_or(""))
}

fn strip_vendor_prefix(raw: &str) -> &str {
    const VENDORS: &[&str] = &[
        "anthropic/",
        "openai/",
        "google/",
        "xai/",
        "meta-llama/",
        "deepseek/",
        "qwen/",
        "mistralai/",
        "openrouter/",
    ];
    for prefix in VENDORS {
        if let Some(rest) = strip_ascii_prefix_ci(raw, prefix) {
            return rest;
        }
    }
    raw
}

fn format_hyphen_words(raw: &str, title_first: bool) -> String {
    raw.split(['-', ' '])
        .enumerate()
        .filter(|(_, p)| !p.is_empty())
        .map(|(idx, part)| {
            if idx == 0 && !title_first {
                part.to_owned()
            } else {
                format_word(part)
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn format_word(word: &str) -> String {
    if word.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return word.to_owned();
    }
    capitalize_ascii(word)
}

fn capitalize_ascii(word: &str) -> String {
    let mut chars = word.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let mut out = String::with_capacity(word.len());
    out.extend(first.to_uppercase());
    out.push_str(&chars.as_str().to_ascii_lowercase());
    out
}
