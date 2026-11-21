//! Pure Safe Rust Wave 5 reducers for thread error banners, secret cards, MCP OAuth sign-in,
//! dictation locks, `composerEmpty` keybinding evaluation/search, and sidebar update countdown.

use t3_contracts::ids::{ProviderThreadId, ThreadId, TurnItemId};
use t3_contracts::mcp_sign_in::{
    McpSignInStatus, OrchestrationMcpSignInNotice, ProviderMcpSignInCompleteInput,
    ProviderMcpSignInStartInput,
};
use t3_surfaces::mobile_review::decline_secret_request_input;

use crate::actions_features::UpsertSecretCardPayload;
use crate::effects::{WebHostEffect, WebUiError, encode_rpc_request_frame_parts};
use crate::state::WebAppState;
use crate::state_features::{
    McpSignInCardState, McpSignInUiStatus, SecretRequestCardEntry, ThreadErrorBanner,
    ThreadErrorSource,
};

/// Records a new thread error occurrence with a strictly monotonic `occurrence_id` so repeat
/// occurrences of the same message reliably display again (`5872b0a488`, `aa6fe390db`).
pub fn record_thread_error(
    state: &mut WebAppState,
    thread_id: &str,
    message: &str,
    source: ThreadErrorSource,
    runtime_key: Option<&str>,
    is_snapshot_resync: bool,
) {
    if is_snapshot_resync
        && source == ThreadErrorSource::Runtime
        && runtime_key.is_some_and(|k| state.features.dismissed_runtime_keys.contains(k))
    {
        return;
    }
    if !is_snapshot_resync && let Some(k) = runtime_key {
        state.features.dismissed_runtime_keys.remove(k);
    }
    state.features.next_error_occurrence_id =
        state.features.next_error_occurrence_id.saturating_add(1);
    let occurrence_id = state.features.next_error_occurrence_id;
    state.features.thread_errors.insert(
        thread_id.to_owned(),
        ThreadErrorBanner {
            occurrence_id,
            source,
            message: message.to_owned(),
            runtime_key: runtime_key.map(str::to_owned),
        },
    );
}

/// Dismisses only the matching `occurrence_id` (or active banner when `None`) without muting
/// subsequent occurrences of identical error copy.
pub fn dismiss_thread_error(state: &mut WebAppState, thread_id: &str, occurrence_id: Option<u64>) {
    let matches = state
        .features
        .thread_errors
        .get(thread_id)
        .is_some_and(|banner| occurrence_id.is_none_or(|id| id == banner.occurrence_id));
    if !matches {
        return;
    }
    if let Some(removed) = state.features.thread_errors.remove(thread_id)
        && removed.source == ThreadErrorSource::Runtime
        && let Some(key) = removed.runtime_key
    {
        state.features.dismissed_runtime_keys.insert(key);
    }
}

/// Registers or updates a multi-value/show-hide secret request card (`0bc9b57b93`).
pub fn upsert_secret_card(state: &mut WebAppState, payload: &UpsertSecretCardPayload) {
    state.features.secret_cards.insert(
        payload.turn_item_id.clone(),
        SecretRequestCardEntry::from_payload(payload),
    );
}

/// Updates an input field value on a secret request card.
pub fn set_secret_field_value(state: &mut WebAppState, turn_item_id: &str, key: &str, value: &str) {
    if let Some(entry) = state.features.secret_cards.get_mut(turn_item_id) {
        entry.card.set_value(key, value);
    }
}

/// Toggles plaintext visibility for a revealable secret field (reverse state: show <-> hide).
pub fn toggle_secret_field_reveal(state: &mut WebAppState, turn_item_id: &str, key: &str) {
    if let Some(entry) = state.features.secret_cards.get_mut(turn_item_id) {
        let _ = entry.card.toggle_reveal(key);
    }
}

/// Builds and dispatches `secrets.answerRequest` for a completed or declined secret card.
pub fn answer_secret_card(
    state: &mut WebAppState,
    turn_item_id: &str,
    decline: bool,
) -> Result<Vec<WebHostEffect>, WebUiError> {
    let Some(entry) = state.features.secret_cards.get(turn_item_id) else {
        return Err(WebUiError::Validation(format!(
            "unknown secret request card: {turn_item_id}"
        )));
    };
    let tid = ThreadId::new(&entry.thread_id).map_err(|e| WebUiError::InvalidId(e.to_string()))?;
    let item_id =
        TurnItemId::new(&entry.turn_item_id).map_err(|e| WebUiError::InvalidId(e.to_string()))?;
    let payload_struct = if decline {
        decline_secret_request_input(tid, item_id)
    } else {
        entry.card.to_answer_input(tid, item_id).ok_or_else(|| {
            WebUiError::Validation("all required secret fields must be filled".to_owned())
        })?
    };
    let req_id = state.client.alloc_request_id();
    let payload = serde_json::to_value(&payload_struct)?;
    let parts = encode_rpc_request_frame_parts(req_id, "secrets.answerRequest", &payload, None)?;
    if let Some(mut_entry) = state.features.secret_cards.get_mut(turn_item_id) {
        mut_entry.submitted = Some(if decline { "declined" } else { "saved" }.to_owned());
    }
    Ok(vec![WebHostEffect::WsSendFrameParts { parts }])
}

/// Registers an MCP OAuth sign-in notice from an orchestration system notice (`669d8941e2`).
pub fn upsert_mcp_sign_in(
    state: &mut WebAppState,
    thread_id: &str,
    notice_id: &str,
    notice: &OrchestrationMcpSignInNotice,
) {
    state.features.mcp_sign_ins.insert(
        notice_id.to_owned(),
        McpSignInCardState {
            notice_id: notice_id.to_owned(),
            thread_id: thread_id.to_owned(),
            provider_thread_id: thread_id.to_owned(),
            server: notice.server.clone(),
            status: McpSignInUiStatus::Pending,
            signed_in_at: None,
            error_message: None,
        },
    );
}

/// Starts an MCP OAuth sign-in flow (`provider.mcpSignIn.start`), emitting WS RPC parts.
pub fn start_mcp_sign_in(
    state: &mut WebAppState,
    notice_id: &str,
    provider_thread_id: Option<&str>,
) -> Result<Vec<WebHostEffect>, WebUiError> {
    let (tid_str, ptid_str, server) = {
        let Some(entry) = state.features.mcp_sign_ins.get_mut(notice_id) else {
            return Err(WebUiError::Validation(format!(
                "unknown MCP sign-in notice: {notice_id}"
            )));
        };
        if let Some(ptid) = provider_thread_id {
            ptid.clone_into(&mut entry.provider_thread_id);
        }
        entry.status = McpSignInUiStatus::Authorizing;
        entry.error_message = None;
        (
            entry.thread_id.clone(),
            entry.provider_thread_id.clone(),
            entry.server.clone(),
        )
    };
    let input = ProviderMcpSignInStartInput {
        thread_id: ThreadId::new(&tid_str).map_err(|e| WebUiError::InvalidId(e.to_string()))?,
        provider_thread_id: ProviderThreadId::new(&ptid_str)
            .map_err(|e| WebUiError::InvalidId(e.to_string()))?,
        server,
    };
    let req_id = state.client.alloc_request_id();
    let payload = serde_json::to_value(&input)?;
    let parts = encode_rpc_request_frame_parts(req_id, "provider.mcpSignIn.start", &payload, None)?;
    Ok(vec![WebHostEffect::WsSendFrameParts { parts }])
}

/// Completes an MCP OAuth callback (`provider.mcpSignIn.complete`) without retaining sensitive
/// callback URL query tokens in client state.
pub fn complete_mcp_sign_in(
    state: &mut WebAppState,
    notice_id: &str,
    callback_url: &str,
    provider_thread_id: Option<&str>,
) -> Result<Vec<WebHostEffect>, WebUiError> {
    if callback_url.trim().is_empty() {
        return Err(WebUiError::Validation(
            "OAuth callback URL must not be empty".to_owned(),
        ));
    }
    let (tid_str, ptid_str, server) = {
        let Some(entry) = state.features.mcp_sign_ins.get_mut(notice_id) else {
            return Err(WebUiError::Validation(format!(
                "unknown MCP sign-in notice: {notice_id}"
            )));
        };
        if let Some(ptid) = provider_thread_id {
            ptid.clone_into(&mut entry.provider_thread_id);
        }
        entry.status = McpSignInUiStatus::Completing;
        entry.error_message = None;
        (
            entry.thread_id.clone(),
            entry.provider_thread_id.clone(),
            entry.server.clone(),
        )
    };
    let input = ProviderMcpSignInCompleteInput {
        thread_id: ThreadId::new(&tid_str).map_err(|e| WebUiError::InvalidId(e.to_string()))?,
        provider_thread_id: ProviderThreadId::new(&ptid_str)
            .map_err(|e| WebUiError::InvalidId(e.to_string()))?,
        server,
        callback_url: callback_url.to_owned(),
    };
    let req_id = state.client.alloc_request_id();
    let payload = serde_json::to_value(&input)?;
    let parts =
        encode_rpc_request_frame_parts(req_id, "provider.mcpSignIn.complete", &payload, None)?;
    Ok(vec![WebHostEffect::WsSendFrameParts { parts }])
}

/// Applies MCP OAuth sign-in completion/cancellation/error results cleanly.
pub fn resolve_mcp_sign_in(
    state: &mut WebAppState,
    notice_id: &str,
    status: Option<McpSignInUiStatus>,
    contract_status: Option<McpSignInStatus>,
    error_message: Option<String>,
) {
    let Some(entry) = state.features.mcp_sign_ins.get_mut(notice_id) else {
        return;
    };
    if let Some(cs) = contract_status {
        match cs {
            McpSignInStatus::Pending { server, .. } => {
                entry.server = server;
                entry.status = McpSignInUiStatus::Pending;
                entry.signed_in_at = None;
            }
            McpSignInStatus::SignedIn {
                server,
                signed_in_at,
            } => {
                entry.server = server;
                entry.status = McpSignInUiStatus::SignedIn;
                entry.signed_in_at = Some(signed_in_at);
            }
        }
    }
    if let Some(ui) = status {
        entry.status = ui;
    }
    entry.error_message = error_message.map(|msg| redact_query_tokens(&msg));
}

fn redact_query_tokens(message: &str) -> String {
    if let Some((prefix, _)) = message.split_once("?code=") {
        format!("{prefix}?[redacted]")
    } else if let Some((prefix, _)) = message.split_once("&code=") {
        format!("{prefix}&[redacted]")
    } else {
        message.to_owned()
    }
}
