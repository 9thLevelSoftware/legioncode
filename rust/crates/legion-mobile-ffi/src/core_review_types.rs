//! `Serde` request and response payloads for mobile review, linked-line, tunnel, and secret-card bridges.

use std::collections::BTreeMap;

use legion_contracts::orchestration::SecretRequestField;
use legion_contracts::policy::ManagedPolicySummary;
use legion_contracts::secrets::SecretRequestAnswer;
use legion_surfaces::mobile_review::{
    FilteredReviewFiles, LinkedLineRange, ReviewFileEntry, ReviewGitSectionKind, SecretInputSpec,
};
use legion_surfaces::mobile_tunnel::TunnelNavigationState;
use serde::{Deserialize, Serialize};

/// Input payload for [`crate::core_review::filter_review_files_json`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilterReviewFilesRequest {
    /// Changed files reported in the review diff.
    #[serde(default)]
    pub files: Vec<ReviewFileEntry>,
    /// When `true`, test and mock files are hidden unless matched by `include_path`.
    #[serde(default)]
    pub hide_test_files: bool,
    /// Optional currently open path kept visible even when classified as a test file.
    #[serde(default)]
    pub include_path: Option<String>,
    /// Optional currently selected file index used to compute adjacent preload patch indices.
    #[serde(default)]
    pub selected_index: Option<usize>,
}

/// Serialized result returned by [`crate::core_review::filter_review_files_json`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FilterReviewFilesResponse {
    /// Visible files, hidden count, and aggregated line totals.
    #[serde(flatten)]
    pub filtered: FilteredReviewFiles,
    /// Up-to-3 visible patch indices anchored at `selected_index` (or top).
    pub shown_patch_indices: Vec<usize>,
}

/// Input payload for [`crate::core_review::linked_lines_json`].
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkedLinesRequest {
    /// First 1-based linked line number (unordered bounds are normalized).
    #[serde(default)]
    pub start_line: Option<u32>,
    /// Last 1-based linked line number (unordered bounds are normalized).
    #[serde(default)]
    pub end_line: Option<u32>,
    /// Convenience single 1-based linked line number when `start_line`/`end_line` are omitted.
    #[serde(default)]
    pub line: Option<u32>,
    /// iOS change marker width in points (`dp`).
    #[serde(default)]
    pub change_bar_width_dp: Option<f32>,
    /// Android change marker width in physical pixels (`px`).
    #[serde(default)]
    pub change_bar_width_px: Option<f32>,
    /// Screen density multiplier (`1.0`, `2.0`, `3.0`).
    #[serde(default)]
    pub density: Option<f32>,
    /// Requested git review section source selector.
    #[serde(default)]
    pub request_source_kind: Option<ReviewGitSectionKind>,
    /// File git review section source selector.
    #[serde(default)]
    pub file_source_kind: Option<ReviewGitSectionKind>,
}

/// Normalized linked-line range and marker widths returned by [`crate::core_review::linked_lines_json`].
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkedLinesResponse {
    /// Normalized inclusive `[startLine, endLine]` highlight range, if lines were requested.
    pub range: Option<LinkedLineRange>,
    /// Highlighted line count (`0` when `range` is `None`).
    pub line_count: u32,
    /// Effective iOS selection marker bar width in `dp` (`>= 4.0`).
    pub marker_width_dp: f32,
    /// Effective Android selection marker bar width in `px` (`>= 4.0 * density`).
    pub marker_width_px: f32,
    /// Validated git review section selector when provided.
    pub source_kind: Option<ReviewGitSectionKind>,
}

/// Header representation accepted by [`TunnelGuardRequest`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum TunnelHeadersInput {
    /// Ordered list of `[name, value]` pairs.
    Pairs(Vec<(String, String)>),
    /// Header map keyed by header name.
    Map(BTreeMap<String, String>),
}

/// Optional preview `WebView` bind parameters checked against settings and policy.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelBindCheckInput {
    /// Owning preview `WebView` identifier.
    pub webview_id: String,
    /// Target environment identifier.
    pub environment_id: String,
    /// Local loopback proxy host.
    pub proxy_host: String,
    /// Local loopback SOCKS listener port.
    pub proxy_port: u16,
    /// Stand-in loopback domain.
    pub stand_in_host: String,
    /// Optional upstream WebSocket tunnel URL.
    #[serde(default)]
    pub tunnel_base_url: Option<String>,
}

/// Input payload for [`crate::core_review::tunnel_guard_json`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelGuardRequest {
    /// User/environment setting enabling loopback preview tunnels.
    #[serde(default = "default_true")]
    pub enable_preview_tunnel: bool,
    /// Optional server managed policy summary.
    #[serde(default)]
    pub managed_policy: Option<ManagedPolicySummary>,
    /// Optional HTTP refusal status code (`502`/`503`/`504`, etc.).
    #[serde(default)]
    pub http_status: Option<u16>,
    /// Optional HTTP refusal response headers.
    #[serde(default)]
    pub headers: Option<TunnelHeadersInput>,
    /// Optional `ScopedWebViewTunnelRegistry::bind_if_allowed` check parameters.
    #[serde(default)]
    pub bind: Option<TunnelBindCheckInput>,
}

/// Tunnel navigation policy and refusal state returned by [`crate::core_review::tunnel_guard_json`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelGuardResponse {
    /// `true` only when effective navigation state is `Allowed`.
    pub allowed: bool,
    /// `true` when preview tunnels are disabled by settings, policy, or refusal header.
    pub tunnel_disabled: bool,
    /// State implied solely by `enable_preview_tunnel` and `managed_policy`.
    pub policy_state: TunnelNavigationState,
    /// State classified from HTTP refusal status/headers, if applicable.
    pub refusal_state: Option<TunnelNavigationState>,
    /// Final combined navigation state (`TunnelDisabled` policy/header wins).
    pub effective_state: TunnelNavigationState,
    /// Result of validating `bind` via `ScopedWebViewTunnelRegistry::bind_if_allowed`, when requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bind_allowed: Option<bool>,
    /// Error reason if `bind` failed validation or was blocked by policy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bind_error: Option<String>,
}

/// Submission action evaluated for a secret card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SecretCardAction {
    /// Save single-value or multi-value secret answer when all fields are non-empty.
    Save,
    /// Decline to supply the requested secret(s).
    Decline,
}

/// Input payload for [`crate::core_review::secret_card_json`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretCardRequest {
    /// Single-value fallback label when `fields` is absent.
    #[serde(default)]
    pub label: String,
    /// Single-value fallback placeholder when `fields` is absent.
    #[serde(default)]
    pub placeholder: Option<String>,
    /// Multi-value field definitions when present.
    #[serde(default)]
    pub fields: Option<Vec<SecretRequestField>>,
    /// `true` when the agent receives a short-lived private file instead of stored credentials.
    #[serde(default)]
    pub delivery_file: bool,
    /// Current typed field values keyed by field `key` (`""` for single-value cards).
    #[serde(default)]
    pub values: BTreeMap<String, String>,
    /// Keys currently toggled into plaintext visibility.
    #[serde(default)]
    pub revealed_keys: Vec<String>,
    /// Optional key whose visibility toggle was just pressed.
    #[serde(default)]
    pub toggle_reveal_key: Option<String>,
    /// Optional explicit action (`"save"` builds answer when ready; `"decline"` returns Decline).
    #[serde(default)]
    pub submit_action: Option<SecretCardAction>,
}

/// Visibility status for one field without echoing secret values unnecessarily.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretFieldViewState {
    /// Empty string for single-value cards; field `name` for multi-value cards.
    pub key: String,
    /// `true` when currently shown in plaintext (non-secret field or toggled revealed).
    pub is_revealed: bool,
    /// Inverse of `is_revealed` (`true` while input must stay masked).
    pub is_masked: bool,
    /// `true` when trimmed input value is non-empty.
    pub has_value: bool,
}

/// Output payload returned by [`crate::core_review::secret_card_json`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretCardResponse {
    /// Normalized field specifications in display order.
    pub inputs: Vec<SecretInputSpec>,
    /// Per-field mask/reveal and completeness flags.
    pub field_states: Vec<SecretFieldViewState>,
    /// `true` when every field has a non-empty trimmed value.
    pub is_ready: bool,
    /// Localized/standard privacy guidance matching `delivery_file`.
    pub privacy_note: String,
    /// Save or Decline answer ready for `secrets.answerRequest`, when applicable.
    pub answer: Option<SecretRequestAnswer>,
}

const fn default_true() -> bool {
    true
}
