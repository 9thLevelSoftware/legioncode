//! Dispatch router for Wave 5 UI feature actions, keeping `update_dispatch.rs` compact.

use t3_client_core::linked_threads::{HistoryLoadStatus, LinkedThreadViewState};

use crate::actions::WebAppAction;
use crate::effects::{WebHostEffect, WebUiError};
use crate::state::WebAppState;
use crate::update_features::{
    answer_secret_card, complete_mcp_sign_in, dismiss_thread_error, record_thread_error,
    resolve_mcp_sign_in, set_secret_field_value, start_mcp_sign_in, toggle_secret_field_reveal,
    upsert_mcp_sign_in, upsert_secret_card,
};
use crate::update_fleet::{
    advance_batch_link, close_linked_pane, open_linked_pane, reopen_linked_pane,
    set_environments_fleet, set_project_repositories, start_batch_link, sync_linked_pane_view,
    toggle_pin_thread_repository, toggle_project_repository,
};
use crate::update_settings_features::tick_update_countdown;

pub(crate) fn dispatch_wave5_action(
    state: &mut WebAppState,
    action: WebAppAction,
) -> Result<Vec<WebHostEffect>, WebUiError> {
    match action {
        WebAppAction::RecordThreadError(_)
        | WebAppAction::DismissThreadError(_)
        | WebAppAction::SetEnvironmentsFleet(_)
        | WebAppAction::SetEnvironmentsMapOpen { .. }
        | WebAppAction::StartBatchLink { .. }
        | WebAppAction::AdvanceBatchLink(_)
        | WebAppAction::OpenLinkedThreadPane(_)
        | WebAppAction::SyncLinkedThreadPane(_)
        | WebAppAction::CloseLinkedThreadPane
        | WebAppAction::ReopenLinkedThreadPane => dispatch_fleet_and_pane(state, action),
        _ => dispatch_cards_and_settings(state, action),
    }
}

fn dispatch_fleet_and_pane(
    state: &mut WebAppState,
    action: WebAppAction,
) -> Result<Vec<WebHostEffect>, WebUiError> {
    match action {
        WebAppAction::RecordThreadError(p) => {
            record_thread_error(
                state,
                &p.thread_id,
                &p.message,
                p.source,
                p.runtime_key.as_deref(),
                false,
            );
            Ok(Vec::new())
        }
        WebAppAction::DismissThreadError(p) => {
            dismiss_thread_error(state, &p.thread_id, p.occurrence_id);
            Ok(Vec::new())
        }
        WebAppAction::SetEnvironmentsFleet(p) => {
            set_environments_fleet(state, p.nodes, p.edges);
            Ok(Vec::new())
        }
        WebAppAction::SetEnvironmentsMapOpen { open } => {
            state.features.environments_map.open = open;
            Ok(Vec::new())
        }
        WebAppAction::StartBatchLink { candidate_ids } => start_batch_link(state, &candidate_ids),
        WebAppAction::AdvanceBatchLink(p) => {
            advance_batch_link(state, &p.environment_id, p.succeeded, p.failure_kind);
            Ok(Vec::new())
        }
        WebAppAction::OpenLinkedThreadPane(p) => {
            open_linked_pane(
                state,
                p.target,
                p.title,
                p.mode,
                p.working,
                p.messages,
                p.has_earlier,
            );
            Ok(Vec::new())
        }
        WebAppAction::SyncLinkedThreadPane(p) => {
            let view = LinkedThreadViewState {
                has_snapshot: true,
                working: p.working.unwrap_or(state.features.linked_pane.working),
                has_more_history: p
                    .has_more_history
                    .unwrap_or(state.features.linked_pane.has_earlier),
                history_status: if p.error_message.is_some() {
                    HistoryLoadStatus::Failed
                } else {
                    HistoryLoadStatus::Idle
                },
                ..LinkedThreadViewState::default()
            };
            sync_linked_pane_view(state, Some(&view), p.error_message.as_deref());
            Ok(Vec::new())
        }
        WebAppAction::CloseLinkedThreadPane => {
            close_linked_pane(state);
            Ok(Vec::new())
        }
        WebAppAction::ReopenLinkedThreadPane => {
            reopen_linked_pane(state);
            Ok(Vec::new())
        }
        _ => Ok(Vec::new()),
    }
}

fn dispatch_cards_and_settings(
    state: &mut WebAppState,
    action: WebAppAction,
) -> Result<Vec<WebHostEffect>, WebUiError> {
    match action {
        WebAppAction::SetRepoPickerOpen { open } => {
            state.features.repo_picker.open = open;
            Ok(Vec::new())
        }
        WebAppAction::SetProjectRepositories(p) => {
            set_project_repositories(
                state,
                &p.project_id,
                p.tracked_folders,
                p.scanned_folders,
                &p.default_worktree_repos,
            );
            Ok(Vec::new())
        }
        WebAppAction::ToggleProjectRepository(p) => {
            toggle_project_repository(state, &p.project_id, &p.folder_path);
            Ok(Vec::new())
        }
        WebAppAction::TogglePinThreadRepository(p) => {
            toggle_pin_thread_repository(state, &p.thread_id, &p.repo_path);
            Ok(Vec::new())
        }
        WebAppAction::UpsertSecretCard(p) => {
            upsert_secret_card(state, &p);
            Ok(Vec::new())
        }
        WebAppAction::SetSecretFieldValue(p) => {
            set_secret_field_value(state, &p.turn_item_id, &p.key, &p.value);
            Ok(Vec::new())
        }
        WebAppAction::ToggleSecretFieldReveal(p) => {
            toggle_secret_field_reveal(state, &p.turn_item_id, &p.key);
            Ok(Vec::new())
        }
        WebAppAction::AnswerSecretCard(p) => answer_secret_card(state, &p.turn_item_id, p.decline),
        WebAppAction::UpsertMcpSignIn(p) => {
            upsert_mcp_sign_in(state, &p.thread_id, &p.notice_id, &p.notice);
            Ok(Vec::new())
        }
        WebAppAction::StartMcpSignIn(p) => {
            start_mcp_sign_in(state, &p.notice_id, p.provider_thread_id.as_deref())
        }
        WebAppAction::CompleteMcpSignIn(p) => complete_mcp_sign_in(
            state,
            &p.notice_id,
            &p.callback_url,
            p.provider_thread_id.as_deref(),
        ),
        WebAppAction::ResolveMcpSignIn(p) => {
            resolve_mcp_sign_in(
                state,
                &p.notice_id,
                p.status,
                p.contract_status,
                p.error_message,
            );
            Ok(Vec::new())
        }
        WebAppAction::SetDictationActive { active } => {
            state.composer.dictating = active;
            Ok(Vec::new())
        }
        WebAppAction::SetKeybindings(p) => {
            state.features.keybindings = p.entries;
            Ok(Vec::new())
        }
        WebAppAction::SearchKeybindings { query } => {
            state.features.keybinding_query = query;
            Ok(Vec::new())
        }
        WebAppAction::SetUpdateCapsule(p) => {
            let cap = &mut state.features.update_capsule;
            cap.phase = p.phase;
            cap.version = p.version;
            cap.progress_percent = p.progress_percent;
            cap.restart_at_ms = p.restart_at_ms;
            cap.dismissed = false;
            cap.error_message = p.error_message;
            tick_update_countdown(state, p.now_ms);
            Ok(Vec::new())
        }
        WebAppAction::TickUpdateCountdown { now_ms } => {
            tick_update_countdown(state, now_ms);
            Ok(Vec::new())
        }
        WebAppAction::PostponeUpdateCapsule => {
            state.features.update_capsule.dismissed = true;
            state.features.update_capsule.phase = crate::state_features::UpdateCapsulePhase::Hidden;
            Ok(Vec::new())
        }
        _ => Ok(Vec::new()),
    }
}
