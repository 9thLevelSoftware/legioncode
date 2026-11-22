//! Mobile composer clipboard context-chip filter (`BUG-23`).
//!
//! Mirrors `LegionComposerClipboard` in `LegionComposerEditorModule.kt` and
//! `LegionComposerEditorModule.swift` while fixing `BUG-23`: missing, `null`, `""`, and
//! whitespace-only `contextId` / `screenshotContextId` values normalize strictly to `None`
//! so `optString("contextId")` never synthesizes `"/{""})"` -> `"/)"` chip patterns that match
//! arbitrary pasted code.

use std::collections::{BTreeMap, BTreeSet};

use serde::de::Deserializer;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Normalized context identifiers for a composer attachment/chip record (`BUG-23`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposerClipboardContext {
    context_id: Option<String>,
    screenshot_context_id: Option<String>,
}

impl ComposerClipboardContext {
    /// Normalizes `None`, `""`, and whitespace-only strings to `None` (`BUG-23`).
    pub fn new(context_id: Option<&str>, screenshot_context_id: Option<&str>) -> Self {
        Self {
            context_id: normalize_optional_id(context_id),
            screenshot_context_id: normalize_optional_id(screenshot_context_id),
        }
    }

    /// Returns the normalized primary `contextId`, or `None` if missing/empty/whitespace.
    pub fn context_id(&self) -> Option<&str> {
        self.context_id.as_deref()
    }

    /// Returns the normalized linked `screenshotContextId`, or `None` if missing/empty/whitespace.
    pub fn screenshot_context_id(&self) -> Option<&str> {
        self.screenshot_context_id.as_deref()
    }

    /// Returns `true` when this context has a valid non-empty `contextId`.
    pub fn has_context_id(&self) -> bool {
        self.context_id.is_some()
    }

    /// Builds the `/<id>)` chip reference marker when `contextId` is `Some(_)`, or `None` when
    /// absent so `"/)"` can never match pasted code (`BUG-23`).
    pub fn chip_pattern(&self) -> Option<String> {
        self.context_id.as_deref().map(|id| format!("/{id})"))
    }

    /// Returns `true` if `text` contains this record's `/<contextId>)` chip reference.
    pub fn matches_text(&self, text: &str) -> bool {
        self.chip_pattern()
            .is_some_and(|pattern| text.contains(&pattern))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawComposerClipboardContext {
    #[serde(default)]
    context_id: Option<String>,
    #[serde(default)]
    screenshot_context_id: Option<String>,
}

impl<'de> Deserialize<'de> for ComposerClipboardContext {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = RawComposerClipboardContext::deserialize(deserializer)?;
        Ok(Self::new(
            raw.context_id.as_deref(),
            raw.screenshot_context_id.as_deref(),
        ))
    }
}

/// Full composer clipboard context record preserving any extra JSON metadata fields.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposerClipboardRecord {
    /// Normalized `(contextId, screenshotContextId)` pair.
    #[serde(flatten)]
    pub context: ComposerClipboardContext,
    /// Additional JSON attributes attached to the context record (`label`, `path`, `kind`, ...).
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl ComposerClipboardRecord {
    /// Builds a record from optional `contextId` and `screenshotContextId` strings.
    pub fn new(context_id: Option<&str>, screenshot_context_id: Option<&str>) -> Self {
        Self {
            context: ComposerClipboardContext::new(context_id, screenshot_context_id),
            extra: BTreeMap::new(),
        }
    }
}

/// Result of filtering a clipboard context fragment against the copied plain text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardFilterResult {
    /// Selected primary and linked-screenshot records in original order without duplicates.
    pub selected_records: Vec<ComposerClipboardRecord>,
    /// Re-serialized JSON fragment payload when `selected_records` is non-empty.
    pub fragment_json: Option<String>,
    /// `<pre data-legion-context-fragment="...">` HTML representation when records matched.
    pub html_fragment: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FragmentEnvelope {
    records: Option<Vec<Value>>,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

/// Filters `records` to only those whose valid `/<contextId>)` appears in `text`, plus any
/// referenced `screenshotContextId` records, ignoring null/empty/whitespace IDs (`BUG-23`).
pub fn filter_clipboard_records(
    text: &str,
    records: &[ComposerClipboardRecord],
) -> Vec<ComposerClipboardRecord> {
    let mut selected = Vec::new();
    let mut selected_indices = BTreeSet::new();
    let mut screenshots = BTreeSet::new();

    for (idx, record) in records.iter().enumerate() {
        if record.context.matches_text(text) {
            if let Some(screenshot_id) = record.context.screenshot_context_id() {
                screenshots.insert(screenshot_id.to_owned());
            }
            selected_indices.insert(idx);
            selected.push(record.clone());
        }
    }

    if !screenshots.is_empty() {
        for (idx, record) in records.iter().enumerate() {
            if selected_indices.contains(&idx) {
                continue;
            }
            if let Some(ctx_id) = record.context.context_id()
                && screenshots.contains(ctx_id)
            {
                selected_indices.insert(idx);
                selected.push(record.clone());
            }
        }
    }

    selected
}

/// Filters a serialized `{ "records": [...] }` JSON fragment against `text` and builds the
/// escaped HTML `<pre data-legion-context-fragment="...">` payload only when real chips matched.
pub fn filter_clipboard_fragment_json(text: &str, fragment_json: &str) -> ClipboardFilterResult {
    let Ok(envelope) = serde_json::from_str::<FragmentEnvelope>(fragment_json) else {
        return empty_filter_result();
    };
    let Some(raw_records) = envelope.records else {
        return empty_filter_result();
    };

    let parsed_records: Vec<ComposerClipboardRecord> = raw_records
        .into_iter()
        .filter_map(|val| serde_json::from_value::<ComposerClipboardRecord>(val).ok())
        .collect();
    let selected = filter_clipboard_records(text, &parsed_records);
    if selected.is_empty() {
        return empty_filter_result();
    }

    let mut out_obj = serde_json::Map::new();
    for (key, value) in envelope.extra {
        out_obj.insert(key, value);
    }
    let Ok(records_value) = serde_json::to_value(&selected) else {
        return empty_filter_result();
    };
    out_obj.insert("records".to_owned(), records_value);

    let Ok(serialized) = serde_json::to_string(&Value::Object(out_obj)) else {
        return empty_filter_result();
    };
    let html = Some(build_clipboard_html(text, &serialized));
    ClipboardFilterResult {
        selected_records: selected,
        fragment_json: Some(serialized),
        html_fragment: html,
    }
}

/// Formats `<pre data-legion-context-fragment="...">escaped</pre>` matching mobile native clipboards.
pub fn build_clipboard_html(text: &str, serialized_fragment_json: &str) -> String {
    let encoded = percent_encode_alnum_unreserved(serialized_fragment_json);
    let escaped = text
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!("<pre data-legion-context-fragment=\"{encoded}\">{escaped}</pre>")
}

fn empty_filter_result() -> ClipboardFilterResult {
    ClipboardFilterResult {
        selected_records: Vec::new(),
        fragment_json: None,
        html_fragment: None,
    }
}

fn normalize_optional_id(raw: Option<&str>) -> Option<String> {
    let trimmed = raw?.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

fn percent_encode_alnum_unreserved(input: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(input.len() * 2);
    for byte in input.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push('%');
            out.push(char::from(HEX[usize::from(byte >> 4)]));
            out.push(char::from(HEX[usize::from(byte & 0x0F)]));
        }
    }
    out
}
