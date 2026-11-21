//! Model-View-Update [`WebAppEngine`] reducer and JSON bridge.

use serde::Serialize;

use crate::actions::{RightPaneTab, WebAppAction};
use crate::effects::{WebHostEffect, WebUiError};
use crate::state::{WebAppState, WebAppStateSnapshot};
use crate::update_dispatch::{
    dispatch_auth, dispatch_composer_diff, dispatch_projects, dispatch_terminal_palette,
};
use crate::update_palette::refresh_command_palette;
use crate::update_projection::apply_stream_item;
use crate::update_stream::{
    handle_poll_steady_shells, handle_receive_ws_frame, handle_step_steady_shells,
    handle_touch_thread_family, handle_ws_connected, handle_ws_disconnected,
};
use crate::update_workspace::handle_create_thread;
use crate::view::render_workspace_html;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DispatchJsonResponse {
    effects: Vec<WebHostEffect>,
    html: String,
    state: WebAppStateSnapshot,
}

/// Pure Safe Rust Model-View-Update engine driving T3 Code's web surface.
#[derive(Debug, Clone)]
pub struct WebAppEngine {
    state: WebAppState,
}

impl WebAppEngine {
    /// Initializes a new [`WebAppEngine`] from `locale_hint` and explicit timestamp `now_ms`.
    pub fn new(locale_hint: &str, now_ms: u64) -> Self {
        let mut state = WebAppState::new(locale_hint, now_ms);
        refresh_command_palette(&mut state);
        Self { state }
    }

    /// Borrows the current [`WebAppState`].
    pub const fn state(&self) -> &WebAppState {
        &self.state
    }

    /// Renders the `XSS`-safe `HTML5` workspace view for the current state.
    pub fn render_html(&self) -> String {
        render_workspace_html(&self.state)
    }

    /// Serializes the current state snapshot as JSON.
    pub fn state_snapshot_json(&self) -> Result<String, WebUiError> {
        Ok(serde_json::to_string(&self.state.snapshot())?)
    }

    /// Dispatches one [`WebAppAction`], returning typed [`WebHostEffect`] commands for the host.
    ///
    /// Invalid inputs (such as malformed `"T3VT"` bytes) are surfaced safely inside state fields
    /// without panicking; use [`Self::try_dispatch`] or [`Self::dispatch_json`] to inspect errors.
    pub fn dispatch(&mut self, action: WebAppAction) -> Vec<WebHostEffect> {
        self.try_dispatch(action).unwrap_or_default()
    }

    /// Dispatches one [`WebAppAction`], returning `Err(WebUiError)` on validation or codec errors.
    pub fn try_dispatch(&mut self, action: WebAppAction) -> Result<Vec<WebHostEffect>, WebUiError> {
        match action {
            WebAppAction::SetLocale { .. }
            | WebAppAction::ToggleSidebar { .. }
            | WebAppAction::SetRightTab { .. }
            | WebAppAction::SetPairingInput { .. }
            | WebAppAction::SubmitPairing { .. }
            | WebAppAction::ApplyPairingSuccess { .. }
            | WebAppAction::ApplyAuthError { .. }
            | WebAppAction::RestoreSessionToken { .. }
            | WebAppAction::SignOut => dispatch_auth(&mut self.state, action),
            WebAppAction::FetchProjects
            | WebAppAction::ApplyProjectsSnapshot { .. }
            | WebAppAction::SelectProject { .. }
            | WebAppAction::SetProjectDraft { .. }
            | WebAppAction::CreateProject { .. }
            | WebAppAction::RenameProject { .. }
            | WebAppAction::DeleteProject { .. } => dispatch_projects(&mut self.state, action),
            WebAppAction::WsConnected { .. }
            | WebAppAction::WsDisconnected { .. }
            | WebAppAction::StepSteadyShells { .. }
            | WebAppAction::PollSteadyShells { .. }
            | WebAppAction::TouchThreadFamily { .. }
            | WebAppAction::ReceiveWsTextFrame { .. }
            | WebAppAction::ReceiveStreamItem { .. }
            | WebAppAction::SelectThread { .. }
            | WebAppAction::CreateThread { .. } => self.dispatch_stream(action),
            WebAppAction::UpdateComposerDraft { .. }
            | WebAppAction::PasteIntoComposer { .. }
            | WebAppAction::AddContextPill { .. }
            | WebAppAction::RemoveContextPill { .. }
            | WebAppAction::SendMessage
            | WebAppAction::LoadUnifiedDiff { .. }
            | WebAppAction::DecorateDiffTokens { .. }
            | WebAppAction::SelectDiffFile { .. } => {
                dispatch_composer_diff(&mut self.state, action)
            }
            _ => {
                let (effects, follow_up) = dispatch_terminal_palette(&mut self.state, action)?;
                if let Some(next_action) = follow_up {
                    return self.try_dispatch(next_action);
                }
                Ok(effects)
            }
        }
    }

    fn dispatch_stream(&mut self, action: WebAppAction) -> Result<Vec<WebHostEffect>, WebUiError> {
        let effects = match action {
            WebAppAction::WsConnected { now_ms } => handle_ws_connected(&mut self.state, now_ms)?,
            WebAppAction::WsDisconnected {
                now_ms,
                approval_pending,
                random,
            } => {
                handle_ws_disconnected(&mut self.state, now_ms, approval_pending, random);
                Vec::new()
            }
            WebAppAction::StepSteadyShells { now_ms, inputs } => {
                handle_step_steady_shells(&mut self.state, now_ms, inputs)?;
                refresh_command_palette(&mut self.state);
                Vec::new()
            }
            WebAppAction::PollSteadyShells {
                now_ms,
                timer_token,
            } => {
                handle_poll_steady_shells(&mut self.state, now_ms, timer_token);
                refresh_command_palette(&mut self.state);
                Vec::new()
            }
            WebAppAction::TouchThreadFamily {
                environment_id,
                thread_id,
                op,
                now_ms,
            } => {
                handle_touch_thread_family(
                    &mut self.state,
                    &environment_id,
                    &thread_id,
                    op,
                    now_ms,
                )?;
                Vec::new()
            }
            WebAppAction::ReceiveWsTextFrame { raw_frame, now_ms } => {
                let out = handle_receive_ws_frame(&mut self.state, &raw_frame, now_ms)?;
                refresh_command_palette(&mut self.state);
                out
            }
            WebAppAction::ReceiveStreamItem {
                thread_id,
                item,
                now_ms,
            } => {
                if let Some(ts) = now_ms {
                    self.state.client.now_ms = self.state.client.now_ms.max(ts);
                }
                apply_stream_item(&mut self.state, thread_id.as_deref(), item);
                refresh_command_palette(&mut self.state);
                Vec::new()
            }
            WebAppAction::SelectThread { thread_id } => {
                self.state.active_thread_id = Some(thread_id);
                self.state.right_tab = RightPaneTab::Chat;
                Vec::new()
            }
            WebAppAction::CreateThread {
                project_id,
                title,
                model,
            } => handle_create_thread(&mut self.state, &project_id, &title, model)?,
            _ => Vec::new(),
        };
        Ok(effects)
    }

    /// Parses one JSON [`WebAppAction`], runs [`Self::try_dispatch`], and returns
    /// `{ "effects": [...], "html": "...", "state": {...} }`.
    pub fn dispatch_json(&mut self, json: &str) -> Result<String, WebUiError> {
        let action: WebAppAction = serde_json::from_str(json)
            .map_err(|err| WebUiError::InvalidActionJson(err.to_string()))?;
        let effects = self.try_dispatch(action)?;
        let resp = DispatchJsonResponse {
            effects,
            html: self.render_html(),
            state: self.state.snapshot(),
        };
        Ok(serde_json::to_string(&resp)?)
    }
}
