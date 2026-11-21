//! Core error type and `BUG-22`..`BUG-24` JSON payload schemas for [`crate::core`].

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use t3_client_core::diff_markdown::Utf16Span;
use t3_surfaces::mobile_diff::{AttributedDiffRun, SyntaxTokenInput};
use t3_surfaces::mobile_i18n::{
    AgentActivityInput, AgentActivityPhase, AgentActivityRowInput, MobileNavigationStrings,
};
use t3_terminal_vt::VtError;
use thiserror::Error;

/// Pure-Rust errors produced by JSON and binary FFI payload bridges.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MobileFfiError {
    /// Input JSON could not be deserialized or output JSON could not be serialized.
    #[error("invalid JSON payload: {0}")]
    InvalidJson(String),
    /// Input arguments violated a domain invariant.
    #[error("invalid input: {0}")]
    InvalidInput(String),
    /// Terminal VT grid/snapshot codec rejected the input.
    #[error("terminal VT error: {0}")]
    Vt(#[from] VtError),
    /// Serialized UTF-8 output unexpectedly contained an interior NUL byte.
    #[error("output string contained an interior NUL byte")]
    InteriorNul,
}

/// Request payload for [`crate::core::decorate_diff_json`] (`BUG-22`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DecorateDiffRequest {
    /// Sequential syntax tokens to convert into [`AttributedDiffRun`] items when `runs` is omitted.
    #[serde(default)]
    pub tokens: Option<Vec<SyntaxTokenInput>>,
    /// Pre-built attributed diff runs preserving font style flags and non-color attributes.
    #[serde(default)]
    pub runs: Option<Vec<AttributedDiffRun>>,
    /// Intra-line UTF-16 word-diff highlight spans `[start, end)`.
    #[serde(default, alias = "wordHighlightRanges")]
    pub word_highlights: Vec<Utf16Span>,
    /// Optional hex background color applied to non-note highlighted runs without an explicit bg.
    #[serde(default)]
    pub highlight_background: Option<String>,
}

/// Response payload returned by [`crate::core::decorate_diff_json`] (`BUG-22`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DecorateDiffResponse {
    /// Decorated and coalesced UTF-16 attributed runs.
    pub runs: Vec<AttributedDiffRun>,
}

/// Request payload for [`crate::core::filter_clipboard_json`] (`BUG-23`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilterClipboardRequest {
    /// Plain-text clipboard content being copied.
    pub text: String,
    /// Serialized `{ "records": [...] }` JSON fragment string.
    #[serde(default)]
    pub fragment_json: Option<String>,
    /// Structured `{ "records": [...] }` JSON fragment object.
    #[serde(default)]
    pub fragment: Option<serde_json::Value>,
    /// Direct list of raw context records wrapped into a `{ "records": [...] }` fragment.
    #[serde(default)]
    pub records: Option<Vec<serde_json::Value>>,
}

/// Request payload when [`crate::core::nav_strings_json`] is given JSON settings (`BUG-24`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NavStringsRequest {
    /// Saved language setting (`"system"`, `"en"`, `"de"`, `"pt-BR"`, `"zh-CN"`, ...).
    #[serde(default, alias = "locale")]
    pub language: String,
    /// Ordered host system locale tags used when `language` is `"system"` or empty.
    #[serde(default)]
    pub system_locales: Vec<String>,
}

/// Response payload returned by [`crate::core::nav_strings_json`] (`BUG-24`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NavStringsResponse {
    /// Complete localized navigation chrome strings.
    #[serde(flatten)]
    pub strings: MobileNavigationStrings,
    /// Regional formatting locale preserved from host preferences.
    pub formatting_locale: String,
}

/// Request payload for [`crate::core::live_activity_json`] (`BUG-24`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveActivityRequest {
    /// Saved language tag (`"system"`, `"fr"`, `"ja"`, `"zh-CN"`, ...).
    #[serde(default, alias = "language")]
    pub locale: Option<String>,
    /// Ordered host system locale tags used when `locale` is `"system"` or omitted.
    #[serde(default)]
    pub system_locales: Vec<String>,
    /// Raw push notification `data` map (`activity_line_0..4`, `activity_phase`, ...).
    #[serde(default)]
    pub push_data: Option<BTreeMap<String, String>>,
    /// Pre-parsed [`AgentActivityInput`] structure.
    #[serde(default)]
    pub input: Option<AgentActivityInput>,
    /// Direct activity rows when neither `pushData` nor `input` is supplied.
    #[serde(default)]
    pub rows: Option<Vec<AgentActivityRowInput>>,
    /// Direct aggregate phase when neither `pushData` nor `input` is supplied.
    #[serde(default)]
    pub phase: Option<AgentActivityPhase>,
    /// Whether the Live Activity is currently active.
    #[serde(default)]
    pub active: Option<bool>,
    /// Optional active agent count override.
    #[serde(default)]
    pub active_count: Option<usize>,
    /// Optional attention count override.
    #[serde(default)]
    pub attention_count: Option<usize>,
    /// Optional fallback title when `rows` is empty.
    #[serde(default)]
    pub fallback_title: Option<String>,
    /// Optional fallback chip label when `phase` is omitted.
    #[serde(default)]
    pub fallback_chip: Option<String>,
}
