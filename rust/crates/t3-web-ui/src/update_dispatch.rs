//! Action dispatch routers keeping every handler concise and under Clippy line limits.

use serde_json::json;

use crate::actions::{RightPaneTab, WebAppAction};
use crate::effects::{WebHostEffect, WebUiError};
use crate::state::WebAppState;
use crate::state_types::{CommandPaletteKind, LocaleState};
use crate::update_palette::{command_palette_execute_action, refresh_command_palette};
use crate::update_panels::{
    handle_add_context_pill, handle_composer_paste, handle_decorate_diff_tokens,
    handle_load_unified_diff, handle_terminal_apply_theme, handle_terminal_export_snapshot,
    handle_terminal_feed, handle_terminal_import_snapshot, handle_terminal_paste,
    handle_terminal_reset,
};
use crate::update_workspace::{
    apply_pairing_success, apply_projects_snapshot, handle_create_project, handle_send_message,
    handle_submit_pairing,
};

pub(crate) fn dispatch_auth(
    state: &mut WebAppState,
    action: WebAppAction,
) -> Result<Vec<WebHostEffect>, WebUiError> {
    let effects = match action {
        WebAppAction::SetLocale {
            language,
            system_locales,
        } => {
            let refs: Vec<&str> = system_locales.iter().map(String::as_str).collect();
            state.locale = LocaleState::resolve(&language, &refs);
            refresh_command_palette(state);
            Vec::new()
        }
        WebAppAction::ToggleSidebar { open } => {
            state.sidebar_open = open.unwrap_or(!state.sidebar_open);
            Vec::new()
        }
        WebAppAction::SetRightTab { tab } => {
            state.right_tab = tab;
            Vec::new()
        }
        WebAppAction::SetPairingInput { value } => {
            state.auth.pairing_input = value;
            state.auth.last_error = None;
            Vec::new()
        }
        WebAppAction::SubmitPairing { client_label } => handle_submit_pairing(state, client_label)?,
        WebAppAction::ApplyPairingSuccess {
            access_token,
            session_id,
            role,
            scopes,
        } => apply_pairing_success(state, access_token, session_id, role, scopes),
        WebAppAction::ApplyAuthError { message } => {
            state.auth.last_error = Some(message);
            Vec::new()
        }
        WebAppAction::RestoreSessionToken { token } => restore_token_effects(state, &token),
        WebAppAction::SignOut => {
            state.auth.bearer_token = None;
            state.auth.paired = false;
            state.auth.session_id = None;
            state.auth.role = None;
            state.auth.scopes.clear();
            vec![WebHostEffect::PersistSessionToken {
                token: String::new(),
            }]
        }
        _ => Vec::new(),
    };
    Ok(effects)
}

fn restore_token_effects(state: &mut WebAppState, token: &str) -> Vec<WebHostEffect> {
    let trimmed = token.trim().to_owned();
    if trimmed.is_empty() {
        return Vec::new();
    }
    state.auth.bearer_token = Some(trimmed.clone());
    state.auth.paired = true;
    state.auth.last_error = None;
    vec![
        WebHostEffect::http_get("/api/projects", Some(trimmed)),
        WebHostEffect::WsConnect {
            url: state.client.ws_url.clone(),
        },
    ]
}

pub(crate) fn dispatch_projects(
    state: &mut WebAppState,
    action: WebAppAction,
) -> Result<Vec<WebHostEffect>, WebUiError> {
    let effects = match action {
        WebAppAction::FetchProjects => vec![WebHostEffect::http_get(
            "/api/projects",
            state.auth.bearer_token.clone(),
        )],
        WebAppAction::ApplyProjectsSnapshot { projects } => {
            apply_projects_snapshot(state, projects);
            refresh_command_palette(state);
            Vec::new()
        }
        WebAppAction::SelectProject { project_id } => {
            state.active_project_id = Some(project_id.clone());
            if let Some(first) = state.threads.values().find(|t| t.project_id == project_id) {
                state.active_thread_id = Some(first.id.clone());
            }
            Vec::new()
        }
        WebAppAction::SetProjectDraft {
            workspace_root,
            title,
        } => {
            if let Some(ws) = workspace_root {
                state.new_project_workspace_root = ws;
            }
            if let Some(t) = title {
                state.new_project_title = t;
            }
            Vec::new()
        }
        WebAppAction::CreateProject {
            workspace_root,
            title,
        } => handle_create_project(state, &workspace_root, title)?,
        WebAppAction::RenameProject { project_id, title } => {
            let payload = json!({
                "type": "project.update",
                "projectId": project_id,
                "title": title,
            });
            vec![WebHostEffect::http_post_json(
                "/api/projects/mutate",
                &payload,
                state.auth.bearer_token.clone(),
            )?]
        }
        WebAppAction::DeleteProject { project_id, force } => {
            let payload = json!({
                "type": "project.delete",
                "projectId": project_id,
                "force": force,
            });
            vec![WebHostEffect::http_post_json(
                "/api/projects/mutate",
                &payload,
                state.auth.bearer_token.clone(),
            )?]
        }
        _ => Vec::new(),
    };
    Ok(effects)
}

pub(crate) fn dispatch_composer_diff(
    state: &mut WebAppState,
    action: WebAppAction,
) -> Result<Vec<WebHostEffect>, WebUiError> {
    let effects = match action {
        WebAppAction::UpdateComposerDraft { text } => {
            state.composer.draft = text;
            Vec::new()
        }
        WebAppAction::PasteIntoComposer {
            raw_text,
            clipboard_fragment_json,
        } => {
            handle_composer_paste(state, &raw_text, clipboard_fragment_json.as_deref());
            Vec::new()
        }
        WebAppAction::AddContextPill {
            context_id,
            screenshot_context_id,
            label,
            path,
        } => {
            handle_add_context_pill(
                state,
                context_id.as_deref(),
                screenshot_context_id.as_deref(),
                label.as_deref(),
                path.as_deref(),
            );
            Vec::new()
        }
        WebAppAction::RemoveContextPill { index } => {
            if index < state.composer.context_pills.len() {
                state.composer.context_pills.remove(index);
            }
            Vec::new()
        }
        WebAppAction::SendMessage => handle_send_message(state)?,
        WebAppAction::LoadUnifiedDiff { patch, switch_tab } => {
            handle_load_unified_diff(state, patch, switch_tab);
            Vec::new()
        }
        WebAppAction::DecorateDiffTokens {
            file_name,
            tokens,
            word_highlights,
            highlight_background,
        } => {
            handle_decorate_diff_tokens(
                state,
                file_name,
                &tokens,
                &word_highlights,
                highlight_background.as_deref(),
            );
            Vec::new()
        }
        WebAppAction::SelectDiffFile { index } => {
            if index < state.diff.files.len() {
                state.diff.selected_file_index = index;
            }
            Vec::new()
        }
        _ => Vec::new(),
    };
    Ok(effects)
}

pub(crate) fn dispatch_terminal_palette(
    state: &mut WebAppState,
    action: WebAppAction,
) -> Result<(Vec<WebHostEffect>, Option<WebAppAction>), WebUiError> {
    let effects = match action {
        WebAppAction::TerminalFeed { text } => {
            handle_terminal_feed(state, text.as_bytes());
            Vec::new()
        }
        WebAppAction::TerminalFeedBytes { bytes } => {
            handle_terminal_feed(state, &bytes);
            Vec::new()
        }
        WebAppAction::TerminalResize { cols, rows } => {
            state.terminal.grid.resize(cols, rows);
            Vec::new()
        }
        WebAppAction::TerminalApplyTheme {
            config_text,
            overrides,
            foreground,
            background,
            cursor_color,
        } => {
            handle_terminal_apply_theme(
                state,
                config_text.as_deref(),
                overrides,
                foreground.as_deref(),
                background.as_deref(),
                cursor_color.as_deref(),
            )?;
            Vec::new()
        }
        WebAppAction::TerminalPaste { raw_text } => handle_terminal_paste(state, &raw_text)?,
        WebAppAction::TerminalExportSnapshot => {
            handle_terminal_export_snapshot(state)?;
            Vec::new()
        }
        WebAppAction::TerminalImportSnapshot { bytes } => {
            handle_terminal_import_snapshot(state, &bytes)?;
            Vec::new()
        }
        WebAppAction::TerminalReset => {
            handle_terminal_reset(state);
            Vec::new()
        }
        WebAppAction::OpenCommandPalette => open_palette_effects(state),
        WebAppAction::CloseCommandPalette => {
            state.command_palette.open = false;
            Vec::new()
        }
        WebAppAction::ToggleCommandPalette => {
            if state.command_palette.open {
                state.command_palette.open = false;
                Vec::new()
            } else {
                open_palette_effects(state)
            }
        }
        WebAppAction::SetCommandPaletteQuery { query } => {
            state.command_palette.query = query;
            refresh_command_palette(state);
            Vec::new()
        }
        WebAppAction::ExecuteCommandPaletteItem { index } => {
            if let Some(follow_up) = command_palette_execute_action(state, index) {
                return Ok((Vec::new(), Some(follow_up)));
            }
            palette_focus_effects(state, index)
        }
        _ => Vec::new(),
    };
    Ok((effects, None))
}

fn open_palette_effects(state: &mut WebAppState) -> Vec<WebHostEffect> {
    state.command_palette.open = true;
    refresh_command_palette(state);
    vec![WebHostEffect::FocusSelector {
        selector: "[data-testid=\"command-palette-input\"]".to_owned(),
    }]
}

fn palette_focus_effects(state: &mut WebAppState, index: Option<usize>) -> Vec<WebHostEffect> {
    let idx = index.unwrap_or(state.command_palette.selected_index);
    match state.command_palette.results.get(idx).map(|c| c.kind) {
        Some(CommandPaletteKind::FocusPairing) => vec![WebHostEffect::FocusSelector {
            selector: "[data-testid=\"pairing-token-input\"]".to_owned(),
        }],
        Some(CommandPaletteKind::FocusComposer) => {
            state.right_tab = RightPaneTab::Chat;
            vec![WebHostEffect::FocusSelector {
                selector: "[data-testid=\"composer-input\"]".to_owned(),
            }]
        }
        _ => Vec::new(),
    }
}
