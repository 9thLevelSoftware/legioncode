//! Client caching and model-selection effort helpers:
//! - Git branch cache retention via `refsDigest` (`e337cee952` / #40385)
//! - Diff panel single-flight per-file patch read deduplication (`d51d8c4766` / #40217)
//! - `thought_level` and `role: "reasoning"` effort reading (`9f9ebf61fb` / #40136)

use std::collections::BTreeMap;

use legion_contracts::ids::EnvironmentId;
use legion_contracts::orchestration::{
    ModelSelection, ProviderOptionSelection, ProviderOptionValue,
};
use serde::{Deserialize, Serialize};

/// Legacy and ACP option IDs recognized as reasoning effort selectors (`9f9ebf61fb` / #40136).
pub const REASONING_EFFORT_OPTION_IDS: &[&str] = &[
    "variant",
    "reasoning_effort",
    "thinking_level",
    "thought_level",
];

/// Tracks per-`(environment_id, cwd)` ref digests from Git status refreshes so cached branch lists
/// stay intact whenever no ref OID moved (`e337cee952` / #40385).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct VcsRefsDigestCache {
    digests: BTreeMap<(EnvironmentId, String), String>,
}

impl VcsRefsDigestCache {
    /// Creates an empty digest cache.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            digests: BTreeMap::new(),
        }
    }

    /// Evaluates whether a status refresh invalidated the cached branch list (`true` = invalidate
    /// and reload branches; `false` = ref OIDs unchanged, keep cached branch list).
    pub fn on_status_refresh_settled(
        &mut self,
        environment_id: &EnvironmentId,
        cwd: &str,
        refs_digest: Option<&str>,
    ) -> bool {
        let key = (environment_id.clone(), cwd.to_owned());
        if let Some(digest) = refs_digest
            && self.digests.get(&key).map(String::as_str) == Some(digest)
        {
            return false;
        }
        match refs_digest {
            Some(digest) => {
                self.digests.insert(key, digest.to_owned());
            }
            None => {
                self.digests.remove(&key);
            }
        }
        true
    }

    /// Returns the last observed `refs_digest` for `(environment_id, cwd)`, if any.
    #[must_use]
    pub fn get(&self, environment_id: &EnvironmentId, cwd: &str) -> Option<&str> {
        self.digests
            .get(&(environment_id.clone(), cwd.to_owned()))
            .map(String::as_str)
    }
}

/// Snapshot of the Diff panel's preview revision state for one `(preview_target, patch)` pair
/// (`d51d8c4766` / #40217).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PreviewRevisionState {
    /// Latest preview revision key returned by `diffPreview`.
    pub key: Option<String>,
    /// Timestamp (ms) when the preview's pending read was first observed while waiting.
    pub requested_at_ms: Option<u64>,
    /// Effective revision token that patch reads subscribe to.
    pub revision: Option<String>,
}

/// Observed state of a single file-patch read alongside a preview refresh (`#40217`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PatchReadSnapshot {
    /// True while the file patch read is still in flight.
    pub waiting: bool,
    /// Timestamp (ms) when a settled `Success` patch read completed, if any.
    pub success_timestamp_ms: Option<u64>,
}

impl PreviewRevisionState {
    /// Updates revision tracking for a file patch. When the preview revision advances *while* or
    /// *after* the patch read already started (`patch.waiting || patch.timestamp >= since`), keeps
    /// the prior `revision` so opening the Diff panel requests each file patch only once (`#40217`).
    pub fn step(
        &mut self,
        preview_waiting: bool,
        preview_key: Option<&str>,
        now_ms: u64,
        patch: PatchReadSnapshot,
    ) -> Option<&str> {
        let next_key = preview_key.map(ToOwned::to_owned);
        let requested_at_ms = if preview_waiting {
            Some(self.requested_at_ms.unwrap_or(now_ms))
        } else {
            None
        };

        let mut revision = next_key.clone();
        if self.key.is_some() && next_key == self.key {
            revision.clone_from(&self.revision);
        } else if self.key.is_some() {
            let since = requested_at_ms.or(self.requested_at_ms);
            let fresh = since.is_some_and(|start| {
                patch.waiting || patch.success_timestamp_ms.is_some_and(|ts| ts >= start)
            });
            if fresh {
                revision.clone_from(&self.revision);
            }
        }

        self.key = next_key;
        self.requested_at_ms = requested_at_ms;
        self.revision = revision;
        self.revision.as_deref()
    }
}

/// Choice inside a model capability select descriptor (`#40136`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderOptionChoice {
    /// Option choice value ID (`"high"`, `"deep"`, ...).
    pub id: String,
    /// User-facing label (`"High"`, `"Deep"`, ...).
    pub label: String,
    /// True if this option is the provider's default choice.
    #[serde(default)]
    pub is_default: bool,
}

/// Minimal descriptor metadata read from `ServerProviderModel.capabilities.optionDescriptors`
/// (`#40136`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderOptionDescriptor {
    /// Option identifier (`"thought_level"`, `"custom_thinking_mode"`, ...).
    pub id: String,
    /// Control type (`"select"`, `"boolean"`, ...).
    #[serde(rename = "type")]
    pub option_type: String,
    /// Semantic role metadata (`Some("reasoning")` recognizes custom ACP effort controls).
    #[serde(default)]
    pub role: Option<String>,
    /// Available choices when `option_type == "select"`.
    #[serde(default)]
    pub options: Vec<ProviderOptionChoice>,
}

/// Reads the raw saved effort value from `selection.options`, preferring any `select` descriptor
/// with `role == "reasoning"` and falling back through [`REASONING_EFFORT_OPTION_IDS`] including
/// `thought_level` (`9f9ebf61fb` / #40136).
#[must_use]
pub fn read_selected_effort_value(
    selection: Option<&ModelSelection>,
    descriptors: &[ProviderOptionDescriptor],
) -> Option<String> {
    let options = selection.and_then(|s| s.options.as_ref())?;
    let candidate_ids = collect_reasoning_option_ids(descriptors);
    for id in candidate_ids {
        if let Some(val) = lookup_selection_string_option(options, &id)
            && val != "default"
        {
            return Some(val);
        }
    }
    None
}

/// Formats the human-readable effort label from `descriptors` (matching `role: "reasoning"` first,
/// then [`REASONING_EFFORT_OPTION_IDS`], `#40136`), falling back to the raw value if unlabeled.
#[must_use]
pub fn format_model_selection_effort(
    selection: Option<&ModelSelection>,
    descriptors: &[ProviderOptionDescriptor],
) -> Option<String> {
    let options = selection.and_then(|s| s.options.as_ref())?;
    for id in collect_reasoning_option_ids(descriptors) {
        let Some(desc) = descriptors
            .iter()
            .find(|d| d.id == id && d.option_type == "select")
        else {
            continue;
        };
        if let Some(val) = lookup_selection_string_option(options, &id)
            && val != "default"
        {
            let label = desc
                .options
                .iter()
                .find(|opt| opt.id == val)
                .map_or(val, |opt| opt.label.clone());
            return Some(label);
        }
    }
    None
}

fn collect_reasoning_option_ids(descriptors: &[ProviderOptionDescriptor]) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for desc in descriptors {
        if desc.option_type == "select"
            && desc.role.as_deref() == Some("reasoning")
            && !ids.contains(&desc.id)
        {
            ids.push(desc.id.clone());
        }
    }
    for &legacy in REASONING_EFFORT_OPTION_IDS {
        if !ids.iter().any(|existing| existing == legacy) {
            ids.push(legacy.to_owned());
        }
    }
    ids
}

fn lookup_selection_string_option(
    options: &[ProviderOptionSelection],
    key: &str,
) -> Option<String> {
    options.iter().find_map(|item| {
        if item.id != key {
            return None;
        }
        match &item.value {
            ProviderOptionValue::Text(raw) => {
                let trimmed = raw.trim();
                (!trimmed.is_empty()).then(|| trimmed.to_owned())
            }
            ProviderOptionValue::Bool(_) => None,
        }
    })
}
