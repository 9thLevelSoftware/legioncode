//! Composer (`BUG-23`), diff (`BUG-22`), and terminal `VT` (`BUG-25`, `"LCVT"`) handlers.

use legion_client_core::diff_markdown::{Utf16Span, parse_unified_diff};
use legion_surfaces::mobile_composer::{ComposerClipboardContext, filter_clipboard_fragment_json};
use legion_surfaces::mobile_diff::{
    SyntaxTokenInput, build_syntax_runs_from_tokens, decorate_diff_runs_with_background,
    decorate_soa_line,
};
use legion_terminal_vt::palette::{TerminalThemeConfig, parse_color};
use legion_terminal_vt::paste::encode_paste;
use legion_terminal_vt::snapshot::try_encode_lcvt;
use legion_terminal_vt::{decode_lcvt, sanitize_bracketed_paste};

use crate::actions::{PaletteOverrideInput, RightPaneTab};
use crate::effects::{WebHostEffect, WebUiError, encode_rpc_request_frame_parts};
use crate::state::WebAppState;
use crate::state_types::{ComposerContextPill, DecoratedDiffFile, DecoratedDiffLine};

pub(crate) fn handle_composer_paste(
    state: &mut WebAppState,
    raw_text: &str,
    clipboard_fragment_json: Option<&str>,
) {
    if state.composer.dictating {
        return;
    }
    let clean = sanitize_bracketed_paste(raw_text);
    state.composer.draft.push_str(&clean);
    state.composer.last_sanitized_paste = Some(clean.clone());

    if let Some(frag_json) = clipboard_fragment_json {
        let filtered = filter_clipboard_fragment_json(&clean, frag_json);
        state.composer.last_clipboard_html = filtered.html_fragment;
        for record in filtered.selected_records {
            if let Some(ctx_id) = record.context.context_id() {
                let already = state
                    .composer
                    .context_pills
                    .iter()
                    .any(|pill| pill.context.context_id() == Some(ctx_id));
                if !already {
                    let label = record
                        .extra
                        .get("label")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or(ctx_id)
                        .to_owned();
                    let path = record
                        .extra
                        .get("path")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned);
                    state.composer.context_pills.push(ComposerContextPill {
                        context: record.context,
                        label,
                        path,
                    });
                }
            }
        }
    }
}

pub(crate) fn handle_add_context_pill(
    state: &mut WebAppState,
    context_id: Option<&str>,
    screenshot_context_id: Option<&str>,
    label: Option<&str>,
    path: Option<&str>,
) {
    let ctx = ComposerClipboardContext::new(context_id, screenshot_context_id);
    let Some(id) = ctx.context_id().map(str::to_owned) else {
        return;
    };
    if state
        .composer
        .context_pills
        .iter()
        .any(|pill| pill.context.context_id() == Some(id.as_str()))
    {
        return;
    }
    let display = label
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(&id)
        .to_owned();
    state.composer.context_pills.push(ComposerContextPill {
        context: ctx,
        label: display,
        path: path
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned),
    });
}

pub(crate) fn handle_load_unified_diff(state: &mut WebAppState, patch: String, switch_tab: bool) {
    let parsed_files = parse_unified_diff(&patch);
    let files = parsed_files
        .into_iter()
        .map(|file| {
            let mut lines = Vec::with_capacity(file.lines.len());
            for line_idx in 0..file.lines.len() {
                if let Some(line) = file.lines.line(line_idx)
                    && let Some(runs) = decorate_soa_line(&file.lines, line_idx)
                {
                    lines.push(DecoratedDiffLine {
                        kind: line.kind,
                        old_line_number: line.old_line_number,
                        new_line_number: line.new_line_number,
                        runs,
                    });
                }
            }
            DecoratedDiffFile {
                name: file.name,
                prev_name: file.prev_name,
                additions: file.additions,
                deletions: file.deletions,
                lines,
            }
        })
        .collect();
    state.diff.raw_patch = patch;
    state.diff.files = files;
    state.diff.selected_file_index = 0;
    if switch_tab {
        state.right_tab = RightPaneTab::Diff;
    }
}

pub(crate) fn handle_decorate_diff_tokens(
    state: &mut WebAppState,
    file_name: String,
    tokens: &[SyntaxTokenInput],
    word_highlights: &[Utf16Span],
    highlight_background: Option<&str>,
) {
    let fallback_tokens;
    let effective_tokens = if tokens.is_empty()
        && let Some(lang) = crate::hdl_highlight::resolve_hdl_language(&file_name)
    {
        let raw_line = state.diff.raw_patch.lines().next().unwrap_or("");
        fallback_tokens = crate::hdl_highlight::tokenize_hdl(lang, raw_line);
        &fallback_tokens[..]
    } else {
        tokens
    };
    let syntax_runs = build_syntax_runs_from_tokens(effective_tokens);
    let bg = highlight_background.unwrap_or("#3b2e1e");
    let runs = decorate_diff_runs_with_background(&syntax_runs, word_highlights, bg);
    let line = DecoratedDiffLine {
        kind: legion_client_core::diff_markdown::DiffLineKind::Add,
        old_line_number: None,
        new_line_number: Some(1),
        runs,
    };
    state.diff.files = vec![DecoratedDiffFile {
        name: file_name,
        prev_name: None,
        additions: 1,
        deletions: 0,
        lines: vec![line],
    }];
    state.diff.selected_file_index = 0;
    state.right_tab = RightPaneTab::Diff;
}

pub(crate) fn handle_terminal_feed(state: &mut WebAppState, bytes: &[u8]) {
    state.terminal.imported_snapshot = None;
    state.terminal.snapshot_error = None;
    let mut parser = std::mem::take(&mut state.terminal.vt_parser);
    parser.feed(bytes, &mut state.terminal.grid);
    state.terminal.vt_parser = parser;
}

pub(crate) fn handle_terminal_apply_theme(
    state: &mut WebAppState,
    config_text: Option<&str>,
    overrides: Vec<PaletteOverrideInput>,
    foreground: Option<&str>,
    background: Option<&str>,
    cursor_color: Option<&str>,
) -> Result<(), WebUiError> {
    let mut theme = match config_text {
        Some(cfg) => TerminalThemeConfig::parse(cfg),
        None => TerminalThemeConfig::new(),
    };
    if let Some(fg) = foreground {
        theme.foreground = Some(parse_color(fg)?);
    }
    if let Some(bg) = background {
        theme.background = Some(parse_color(bg)?);
    }
    if let Some(cc) = cursor_color {
        theme.cursor_color = Some(parse_color(cc)?);
    }
    if !overrides.is_empty() || config_text.is_none() {
        let mut pairs = Vec::with_capacity(overrides.len());
        for entry in overrides {
            pairs.push((entry.index, parse_color(&entry.color)?));
        }
        theme.palette.apply_theme_overrides(&pairs);
    }
    state.terminal.palette = theme.palette.clone();
    state.terminal.grid.apply_theme(&theme);
    if let Some(ref mut snap) = state.terminal.imported_snapshot {
        snap.palette = theme.palette;
    }
    state.terminal.snapshot_error = None;
    Ok(())
}

pub(crate) fn handle_terminal_paste(
    state: &mut WebAppState,
    raw_text: &str,
) -> Result<Vec<WebHostEffect>, WebUiError> {
    let sanitized = sanitize_bracketed_paste(raw_text);
    let encoded = encode_paste(raw_text, state.terminal.grid.bracketed_paste_enabled());
    state.terminal.input_draft = sanitized;
    let req_id = state.client.alloc_request_id();
    let payload = serde_json::json!({
        "threadId": state.active_thread_id.as_deref().unwrap_or("local"),
        "data": encoded,
    });
    let parts = encode_rpc_request_frame_parts(req_id, "terminal.write", &payload, None)?;
    Ok(vec![WebHostEffect::WsSendFrameParts { parts }])
}

pub(crate) fn handle_terminal_export_snapshot(state: &mut WebAppState) -> Result<(), WebUiError> {
    let snap = state.terminal.active_snapshot();
    let encoded = try_encode_lcvt(&snap)?;
    state.terminal.last_exported_lcvt = Some(encoded);
    state.terminal.snapshot_error = None;
    Ok(())
}

pub(crate) fn handle_terminal_import_snapshot(
    state: &mut WebAppState,
    bytes: &[u8],
) -> Result<(), WebUiError> {
    match decode_lcvt(bytes) {
        Ok(snap) => {
            state.terminal.grid.resize(snap.cols, snap.rows);
            let pairs = snap.palette.override_pairs();
            state.terminal.palette.apply_theme_overrides(&pairs);
            state
                .terminal
                .grid
                .palette_mut()
                .apply_theme_overrides(&pairs);
            state.terminal.imported_snapshot = Some(snap);
            state.terminal.snapshot_error = None;
            state.right_tab = RightPaneTab::Terminal;
            Ok(())
        }
        Err(err) => {
            state.terminal.snapshot_error = Some(err.to_string());
            Err(WebUiError::InvalidSnapshot(err))
        }
    }
}

pub(crate) fn handle_terminal_reset(state: &mut WebAppState) {
    state.terminal.vt_parser.reset();
    state.terminal.grid.reset();
    state.terminal.palette.clear();
    state.terminal.imported_snapshot = None;
    state.terminal.input_draft.clear();
    state.terminal.last_exported_lcvt = None;
    state.terminal.snapshot_error = None;
}
