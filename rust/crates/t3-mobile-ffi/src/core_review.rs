//! Pure Safe Rust bridges for mobile review filtering, linked-line highlights, tunnel guards,
//! and multi-value secret cards backed by [`t3_surfaces`].

use t3_contracts::secrets::SecretRequestAnswer;
use t3_surfaces::mobile_review::{
    LinkedLineRange, SecretCardState, filter_review_files, secret_request_privacy_note,
    selection_marker_width_dp, selection_marker_width_px, shown_patch_indices,
    validate_review_source_selectors,
};
use t3_surfaces::mobile_tunnel::{
    ScopedWebViewTunnelRegistry, TunnelBindSpec, TunnelNavigationState,
    classify_tunnel_http_refusal, evaluate_tunnel_navigation,
};

use crate::core_review_types::{
    FilterReviewFilesRequest, FilterReviewFilesResponse, LinkedLinesRequest, LinkedLinesResponse,
    SecretCardAction, SecretCardRequest, SecretCardResponse, SecretFieldViewState,
    TunnelGuardRequest, TunnelGuardResponse, TunnelHeadersInput,
};
use crate::core_types::MobileFfiError;

/// Filters review diff files (hiding test/mock artifacts unless explicitly kept) and computes
/// next preload patch indices (`filter_review_files` / `shown_patch_indices`).
pub fn filter_review_files_json(input_json: &str) -> Result<String, MobileFfiError> {
    let req: FilterReviewFilesRequest = serde_json::from_str(input_json)
        .map_err(|err| MobileFfiError::InvalidJson(err.to_string()))?;
    let filtered =
        filter_review_files(&req.files, req.hide_test_files, req.include_path.as_deref());
    let paths: Vec<&str> = req.files.iter().map(|entry| entry.path.as_str()).collect();
    let shown_patch_indices = shown_patch_indices(&paths, req.hide_test_files, req.selected_index);
    serde_json::to_string(&FilterReviewFilesResponse {
        filtered,
        shown_patch_indices,
    })
    .map_err(|err| MobileFfiError::InvalidJson(err.to_string()))
}

/// Normalizes linked source-line ranges, computes minimum visible marker bar widths (`>= 4dp`),
/// and validates git review section source selectors.
pub fn linked_lines_json(input_json: &str) -> Result<String, MobileFfiError> {
    let req: LinkedLinesRequest = serde_json::from_str(input_json)
        .map_err(|err| MobileFfiError::InvalidJson(err.to_string()))?;
    let source_kind =
        validate_review_source_selectors(req.request_source_kind, req.file_source_kind)
            .map_err(|msg| MobileFfiError::InvalidInput(msg.to_owned()))?;
    let range = match (req.start_line, req.end_line, req.line) {
        (Some(start), Some(end), _) => Some((start, end)),
        (Some(start), None, _) => Some((start, start)),
        (None, Some(end), _) => Some((end, end)),
        (None, None, Some(single)) => Some((single, single)),
        (None, None, None) => None,
    }
    .map(|(a, b)| {
        if a == 0 || b == 0 {
            Err(MobileFfiError::InvalidInput(
                "linked line numbers must be 1-based".to_owned(),
            ))
        } else {
            Ok(LinkedLineRange::new(a, b))
        }
    })
    .transpose()?;
    let line_count = range.map_or(0, LinkedLineRange::line_count);
    let marker_width_dp = selection_marker_width_dp(req.change_bar_width_dp.unwrap_or(0.0));
    let marker_width_px = selection_marker_width_px(
        req.change_bar_width_px.unwrap_or(0.0),
        req.density.unwrap_or(1.0),
    );
    serde_json::to_string(&LinkedLinesResponse {
        range,
        line_count,
        marker_width_dp,
        marker_width_px,
        source_kind,
    })
    .map_err(|err| MobileFfiError::InvalidJson(err.to_string()))
}

/// Evaluates loopback preview tunnel settings/policy guards and classifies HTTP refusal responses
/// without losing `TunnelDisabled` state across settings, headers, or `WebView` binding.
pub fn tunnel_guard_json(input_json: &str) -> Result<String, MobileFfiError> {
    let req: TunnelGuardRequest = serde_json::from_str(input_json)
        .map_err(|err| MobileFfiError::InvalidJson(err.to_string()))?;
    let policy_state =
        evaluate_tunnel_navigation(req.enable_preview_tunnel, req.managed_policy.as_ref());
    let header_pairs: Vec<(&str, &str)> = match req.headers.as_ref() {
        Some(TunnelHeadersInput::Pairs(pairs)) => pairs
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect(),
        Some(TunnelHeadersInput::Map(map)) => {
            map.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect()
        }
        None => Vec::new(),
    };
    let refusal_state = req
        .http_status
        .and_then(|status| classify_tunnel_http_refusal(status, header_pairs));
    let effective_state = if matches!(policy_state, TunnelNavigationState::TunnelDisabled) {
        TunnelNavigationState::TunnelDisabled
    } else {
        refusal_state.unwrap_or(policy_state)
    };
    let (bind_allowed, bind_error) = if let Some(bind) = req.bind.as_ref() {
        let mut registry = ScopedWebViewTunnelRegistry::new();
        let spec = TunnelBindSpec {
            webview_id: &bind.webview_id,
            environment_id: &bind.environment_id,
            proxy_host: &bind.proxy_host,
            proxy_port: bind.proxy_port,
            stand_in_host: &bind.stand_in_host,
            tunnel_base_url: bind.tunnel_base_url.as_deref(),
        };
        match registry.bind_if_allowed(
            &spec,
            req.enable_preview_tunnel,
            req.managed_policy.as_ref(),
        ) {
            Ok(_) => (Some(true), None),
            Err(err) => (Some(false), Some(err.to_string())),
        }
    } else {
        (None, None)
    };
    serde_json::to_string(&TunnelGuardResponse {
        allowed: matches!(effective_state, TunnelNavigationState::Allowed),
        tunnel_disabled: matches!(effective_state, TunnelNavigationState::TunnelDisabled),
        policy_state,
        refusal_state,
        effective_state,
        bind_allowed,
        bind_error,
    })
    .map_err(|err| MobileFfiError::InvalidJson(err.to_string()))
}

/// Evaluates multi-value and single-value secret request cards, field show/hide masking toggles,
/// and `secrets.answerRequest` payloads (`0bc9b57b93` #31257).
pub fn secret_card_json(input_json: &str) -> Result<String, MobileFfiError> {
    let req: SecretCardRequest = serde_json::from_str(input_json)
        .map_err(|err| MobileFfiError::InvalidJson(err.to_string()))?;
    let mut card = SecretCardState::new(
        &req.label,
        req.placeholder.as_deref(),
        req.fields.as_deref(),
    );
    for (key, value) in &req.values {
        card.set_value(key, value);
    }
    for key in &req.revealed_keys {
        if !card.is_revealed(key) {
            let _ = card.toggle_reveal(key);
        }
    }
    if let Some(toggle_key) = req.toggle_reveal_key.as_deref() {
        let _ = card.toggle_reveal(toggle_key);
    }
    let field_states = card
        .inputs()
        .iter()
        .map(|spec| {
            let is_revealed = card.is_revealed(&spec.key);
            SecretFieldViewState {
                key: spec.key.clone(),
                is_revealed,
                is_masked: !is_revealed,
                has_value: !card.value(&spec.key).trim().is_empty(),
            }
        })
        .collect();
    let answer = match req.submit_action {
        Some(SecretCardAction::Decline) => Some(SecretRequestAnswer::Decline),
        None | Some(SecretCardAction::Save) => card.to_save_answer(),
    };
    serde_json::to_string(&SecretCardResponse {
        inputs: card.inputs().to_vec(),
        field_states,
        is_ready: card.is_ready(),
        privacy_note: secret_request_privacy_note(req.delivery_file).to_owned(),
        answer,
    })
    .map_err(|err| MobileFfiError::InvalidJson(err.to_string()))
}
