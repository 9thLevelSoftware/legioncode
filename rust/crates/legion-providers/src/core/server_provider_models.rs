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
            ("gpt-5.6-sol", "GPT-5.6 Sol", true, false),
            ("gpt-6-luna", "GPT-6 Luna", true, false),
        ],
        "claudeAgent" => &[
            ("claude-opus-5-5", "Claude Opus 5.5", true, true),
            ("claude-sonnet-4-5", "Claude Sonnet 4.5", true, false),
        ],
        "cursor" => &[("auto", "Auto", false, true)],
        "grok" => &[("grok-build", "Grok Build", true, true)],
        "opencode" => &[("openai/gpt-5", "OpenAI GPT-5", true, true)],
        "antigravity" => &[(
            "antigravity-default",
            "Antigravity Session Model",
            true,
            true,
        )],
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
