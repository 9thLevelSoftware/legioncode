//! Pure Safe Rust JSON and binary bridges powering [`crate::ffi`].
//!
//! All domain computation delegates directly to [`legion_surfaces`] (`BUG-22`..`BUG-24`, `BUG-26`),
//! [`legion_terminal_vt`] (`BUG-25`, `"LCVT"`), and [`crate::codegen`] with zero `unsafe` blocks.

use legion_surfaces::mobile_composer::{ClipboardFilterResult, filter_clipboard_fragment_json};
use legion_surfaces::mobile_diff::{
    build_syntax_runs_from_tokens, decorate_diff_runs, decorate_diff_runs_with_background,
};
use legion_surfaces::mobile_i18n::{
    AgentActivityInput, AgentLiveActivityPresenter, MobileNavigationPresenter,
};

use crate::codegen::generate_sdk_sources;

pub use crate::core_tunnel_types::{
    ChunkInput, HostSwapChunksRequest, HostSwapChunksResponse, ThemePlanAction, ThemePlanOperation,
    ThemePlanRequest, ThemePlanResponse, ThemePlanStepOutput,
};
pub use crate::core_types::{
    DecorateDiffRequest, DecorateDiffResponse, FilterClipboardRequest, LiveActivityRequest,
    MobileFfiError, NavStringsRequest, NavStringsResponse,
};
pub use crate::core_vt_types::{
    PaletteEntryOutput, PaletteOverrideInput, PaletteResolveRequest, PaletteResolveResponse,
    RgbaOutput, VtDecodedCellOutput, VtDecodedSnapshotResponse, VtRenderSnapshotRequest,
};

/// Decorates review-diff syntax tokens or runs across UTF-16 word highlights (`BUG-22`).
pub fn decorate_diff_json(input_json: &str) -> Result<String, MobileFfiError> {
    let req: DecorateDiffRequest = serde_json::from_str(input_json)
        .map_err(|err| MobileFfiError::InvalidJson(err.to_string()))?;

    let base_runs = if let Some(runs) = req.runs {
        runs
    } else if let Some(tokens) = req.tokens {
        build_syntax_runs_from_tokens(&tokens)
    } else {
        Vec::new()
    };

    let runs = match req.highlight_background.as_deref() {
        Some(bg) => decorate_diff_runs_with_background(&base_runs, &req.word_highlights, bg),
        None => decorate_diff_runs(&base_runs, &req.word_highlights),
    };

    serde_json::to_string(&DecorateDiffResponse { runs })
        .map_err(|err| MobileFfiError::InvalidJson(err.to_string()))
}

/// Filters composer clipboard context records against copied plain text (`BUG-23`).
pub fn filter_clipboard_json(input_json: &str) -> Result<String, MobileFfiError> {
    let req: FilterClipboardRequest = serde_json::from_str(input_json)
        .map_err(|err| MobileFfiError::InvalidJson(err.to_string()))?;

    let result: ClipboardFilterResult = if let Some(raw_fragment) = req.fragment_json.as_deref() {
        filter_clipboard_fragment_json(&req.text, raw_fragment)
    } else if let Some(fragment_val) = req.fragment {
        let serialized = serde_json::to_string(&fragment_val)
            .map_err(|err| MobileFfiError::InvalidJson(err.to_string()))?;
        filter_clipboard_fragment_json(&req.text, &serialized)
    } else if let Some(records) = req.records {
        let serialized = serde_json::to_string(&serde_json::json!({ "records": records }))
            .map_err(|err| MobileFfiError::InvalidJson(err.to_string()))?;
        filter_clipboard_fragment_json(&req.text, &serialized)
    } else {
        ClipboardFilterResult {
            selected_records: Vec::new(),
            fragment_json: None,
            html_fragment: None,
        }
    };

    serde_json::to_string(&result).map_err(|err| MobileFfiError::InvalidJson(err.to_string()))
}

/// Resolves localized mobile navigation chrome strings for a locale tag or JSON settings (`BUG-24`).
pub fn nav_strings_json(locale_or_json: &str) -> Result<String, MobileFfiError> {
    let trimmed = locale_or_json.trim();
    let presenter = if trimmed.starts_with('{') {
        let req: NavStringsRequest = serde_json::from_str(trimmed)
            .map_err(|err| MobileFfiError::InvalidJson(err.to_string()))?;
        let sys_refs: Vec<&str> = req.system_locales.iter().map(String::as_str).collect();
        MobileNavigationPresenter::from_settings(&req.language, &sys_refs)
    } else if trimmed.starts_with('"') {
        let tag: String = serde_json::from_str(trimmed)
            .map_err(|err| MobileFfiError::InvalidJson(err.to_string()))?;
        MobileNavigationPresenter::for_locale(&tag)
    } else {
        MobileNavigationPresenter::for_locale(locale_or_json)
    };

    let response = NavStringsResponse {
        strings: presenter.strings(),
        formatting_locale: presenter.formatting_locale().to_owned(),
    };
    serde_json::to_string(&response).map_err(|err| MobileFfiError::InvalidJson(err.to_string()))
}

/// Renders localized Live Activity / ongoing notification metadata (`BUG-24`).
pub fn live_activity_json(input_json: &str) -> Result<String, MobileFfiError> {
    let req: LiveActivityRequest = serde_json::from_str(input_json)
        .map_err(|err| MobileFfiError::InvalidJson(err.to_string()))?;

    let language = req.locale.as_deref().unwrap_or("system");
    let sys_refs: Vec<&str> = req.system_locales.iter().map(String::as_str).collect();
    let presenter = AgentLiveActivityPresenter::from_settings(language, &sys_refs);

    let activity_input = if let Some(push_data) = req.push_data.as_ref() {
        AgentActivityInput::from_push_data(push_data, req.active.unwrap_or(true))
    } else if let Some(input) = req.input {
        input
    } else {
        AgentActivityInput {
            rows: req.rows.unwrap_or_default(),
            phase: req.phase,
            active: req.active.unwrap_or(false),
            active_count: req.active_count,
            attention_count: req.attention_count,
            fallback_title: req.fallback_title,
            fallback_chip: req.fallback_chip,
        }
    };

    let presentation = presenter.present(&activity_input);
    serde_json::to_string(&presentation).map_err(|err| MobileFfiError::InvalidJson(err.to_string()))
}

/// Resolves sparse 256-color ANSI palette overrides and xterm-256 fallbacks (`BUG-25`).
pub fn palette_resolve_json(input_json: &str) -> Result<String, MobileFfiError> {
    crate::core_vt::palette_resolve_json(input_json)
}

/// Parses ANSI/VT input with [`legion_terminal_vt::VtParser`] and encodes a `"LCVT"` binary snapshot.
pub fn vt_render_snapshot(input_json: &str) -> Result<Vec<u8>, MobileFfiError> {
    crate::core_vt::vt_render_snapshot(input_json)
}

/// Decodes `"LCVT"` binary snapshot bytes (`v1` or `v2`) into structured JSON.
pub fn vt_decode_snapshot_json(bytes: &[u8]) -> Result<String, MobileFfiError> {
    crate::core_vt::vt_decode_snapshot_json(bytes)
}

/// Streams HTTP chunks through [`legion_surfaces::mobile_tunnel::HttpHeaderHostSwap`] (`BUG-26`).
pub fn host_swap_chunks_json(input_json: &str) -> Result<String, MobileFfiError> {
    crate::core_tunnel::host_swap_chunks_json(input_json)
}

/// Plans per-pane content-addressed Ghostty theme files (`BUG-26`).
pub fn theme_plan_json(input_json: &str) -> Result<String, MobileFfiError> {
    crate::core_tunnel::theme_plan_json(input_json)
}

/// Generates and serializes C header, Swift, and Kotlin/JNA SDK wrapper sources as JSON.
pub fn sdk_sources_json() -> Result<String, MobileFfiError> {
    serde_json::to_string(&generate_sdk_sources())
        .map_err(|err| MobileFfiError::InvalidJson(err.to_string()))
}

/// Generates and serializes the Apple `XCFramework` and `Android` `Gradle`/`NDK` packaging bundle.
pub fn packaging_bundle_json() -> Result<String, MobileFfiError> {
    crate::mobile_packaging::packaging_bundle_json()
}

pub use crate::core_review::{
    filter_review_files_json, linked_lines_json, secret_card_json, tunnel_guard_json,
};
pub use crate::core_review_types::{
    FilterReviewFilesRequest, FilterReviewFilesResponse, LinkedLinesRequest, LinkedLinesResponse,
    SecretCardAction, SecretCardRequest, SecretCardResponse, SecretFieldViewState,
    TunnelBindCheckInput, TunnelGuardRequest, TunnelGuardResponse, TunnelHeadersInput,
};
