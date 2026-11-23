//! Default built-in `ServerProviderModel` catalogs and option capability descriptors.

use crate::core::catalog::{CatalogModelEntry, ModelCatalog, model_short_name};
use crate::core::server_provider::{
    ModelCapabilities, ProviderOptionChoice, ProviderOptionDescriptor, ServerProviderModel,
};

/// Returns normalized built-in [`ServerProviderModel`] entries matching `packages/contracts/src/model.ts`.
#[must_use]
pub fn default_models_for_driver(driver_slug: &str) -> Vec<ServerProviderModel> {
    let entries: &[(&str, &str, bool, bool)] = match driver_slug {
        "codex" => &[
            ("gpt-6-astra", "GPT-6 Astra", true, true),
            ("gpt-6.1-sol", "GPT-6.1 Sol", true, false),
            ("gpt-6-luna", "GPT-6 Luna", true, false),
            ("gpt-5.6-sol", "GPT-5.6 Sol", true, false),
            ("gpt-5.4-mini", "GPT-5.4 Mini", true, false),
        ],
        "claudeAgent" => &[
            ("claude-opus-5-5", "Claude Opus 5.5", true, true),
            ("claude-sonnet-5-5", "Claude Sonnet 5.5", true, false),
            ("claude-fable-5-2", "Claude Fable 5.2", true, false),
            ("claude-haiku-5-5", "Claude Haiku 5.5", true, false),
            ("claude-sonnet-4-6", "Claude Sonnet 4.6", true, false),
            ("claude-haiku-4-5", "Claude Haiku 4.5", true, false),
        ],
        "cursor" => &[("auto", "Auto", false, true)],
        "grok" => &[("grok-build", "Grok Build", true, true)],
        "opencode" => &[("openai/gpt-5", "OpenAI GPT-5", true, true)],
        "antigravity" => &[
            (
                "antigravity-default",
                "Antigravity Session Model",
                true,
                true,
            ),
            ("gemini-4-carbon", "Gemini 4 Carbon", true, false),
            ("gemini-4-argon", "Gemini 4 Argon", true, false),
            (
                "gemini-3.8-flash-high",
                "Gemini 3.8 Flash (High)",
                true,
                false,
            ),
            ("gemini-3.8-flash", "Gemini 3.8 Flash", true, false),
        ],
        "devReplay" => &[("dev-replay", "Replay (dev)", false, true)],
        _ => &[("default", "Default", false, true)],
    };

    entries
        .iter()
        .map(|&(slug, name, reasoning, is_default)| {
            let mut model: ServerProviderModel = ModelCatalog::to_server_model(CatalogModelEntry {
                slug: slug.to_owned(),
                name: name.to_owned(),
                is_custom: false,
                supports_reasoning: reasoning,
            })
            .into();
            model.short_name = Some(model_short_name(name));
            model.is_default = Some(is_default);
            model
        })
        .collect()
}

/// Resolves a catalog model slug into an optional CLI `--model` argument (`None` when using driver default).
#[must_use]
pub fn resolve_cli_model_arg(driver_slug: &str, selected_model: &str) -> Option<String> {
    let trimmed = selected_model.trim();
    if trimmed.is_empty() || trimmed == "default" {
        return None;
    }
    match driver_slug {
        "antigravity" if trimmed == "antigravity-default" => None,
        "grok" if trimmed == "grok-build" => None,
        "claudeAgent" => match trimmed {
            "claude-sonnet-4-5" => Some("sonnet".to_owned()),
            _ => Some(trimmed.to_owned()),
        },
        _ => Some(trimmed.to_owned()),
    }
}

pub(crate) fn reasoning_effort_descriptor() -> ProviderOptionDescriptor {
    ProviderOptionDescriptor::Select {
        id: "reasoning".to_owned(),
        label: "Reasoning Effort".to_owned(),
        description: None,
        options: vec![
            ProviderOptionChoice {
                id: "low".to_owned(),
                label: "Low".to_owned(),
                description: None,
                is_default: None,
            },
            ProviderOptionChoice {
                id: "medium".to_owned(),
                label: "Medium".to_owned(),
                description: None,
                is_default: Some(true),
            },
            ProviderOptionChoice {
                id: "high".to_owned(),
                label: "High".to_owned(),
                description: None,
                is_default: None,
            },
        ],
        current_value: Some("medium".to_owned()),
        role: Some("reasoning".to_owned()),
    }
}

pub(crate) fn capabilities_for_reasoning(supports_reasoning: bool) -> Option<ModelCapabilities> {
    supports_reasoning.then(|| ModelCapabilities {
        option_descriptors: Some(vec![reasoning_effort_descriptor()]),
    })
}

/// Candidate snapshot entry evaluated by [`select_text_generation_fallback`] (`#40699`).
#[derive(Debug, Clone)]
pub struct TextGenerationFallbackCandidate<'a> {
    /// Target provider instance identifier.
    pub instance_id: &'a str,
    /// Underlying provider driver slug.
    pub driver: &'a str,
    /// Candidate model slug.
    pub model_slug: &'a str,
    /// Explicit per-model text generation capability flag (`Some(false)` excludes it).
    pub supports_text_generation: Option<bool>,
    /// Whether this instance+model is available, installed, healthy, and within quota.
    pub usable: bool,
}

/// Selects a fallback `(instance_id, model_slug)` when the primary text-generation selection
/// cannot run (`#40699`).
///
/// Mirrors upstream `TextGeneration.ts`: prefers qualified models on OTHER instances first,
/// only falling back to other usable models on the rejected instance if none exists, and never
/// picks any model whose `supports_text_generation == Some(false)`.
#[must_use]
pub fn select_text_generation_fallback<'a>(
    candidates: &[TextGenerationFallbackCandidate<'a>],
    rejected_instance_id: &str,
    rejected_model_slug: &str,
) -> Option<(&'a str, &'a str)> {
    let other_instance = candidates.iter().find(|cand| {
        cand.usable
            && cand.supports_text_generation != Some(false)
            && cand.instance_id != rejected_instance_id
    });
    if let Some(found) = other_instance {
        return Some((found.instance_id, found.model_slug));
    }
    candidates
        .iter()
        .find(|cand| {
            cand.usable
                && cand.supports_text_generation != Some(false)
                && cand.instance_id == rejected_instance_id
                && cand.model_slug != rejected_model_slug
        })
        .map(|found| (found.instance_id, found.model_slug))
}
