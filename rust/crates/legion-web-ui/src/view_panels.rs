//! `XSS`-safe `HTML5` renderers for Diff (`BUG-22`) and `VT` Terminal (`BUG-25`).

use std::fmt::Write as _;

use legion_client_core::diff_markdown::DiffLineKind;
use legion_surfaces::mobile_diff::AttributedDiffRun;
use legion_terminal_vt::abi::GhosttyStyleFlags;
use legion_terminal_vt::{SnapshotCell, TerminalSnapshot};

use crate::state::{WebAppState, format_rgba_hex};
use crate::state_types::DecoratedDiffFile;
use crate::view::escape_html;

pub(crate) fn sanitize_hex_color(raw: &str) -> Option<&str> {
    let trimmed = raw.trim();
    let hex = trimmed.strip_prefix('#')?;
    if !matches!(hex.len(), 3 | 4 | 6 | 8) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some(trimmed)
}

pub(crate) fn render_diff_panel(out: &mut String, state: &WebAppState) {
    out.push_str(r#"<section class="lc-panel lc-diff-panel" data-testid="diff-tab">"#);
    if state.diff.files.is_empty() {
        let _ = write!(
            out,
            r#"<div class="lc-empty-state" data-testid="diff-empty">{}</div></section>"#,
            escape_html(state.locale.web.no_diff_loaded)
        );
        return;
    }
    out.push_str(r#"<div class="lc-diff-files" data-testid="diff-file-tabs">"#);
    for (idx, file) in state.diff.files.iter().enumerate() {
        let active_cls = if idx == state.diff.selected_file_index {
            " is-active"
        } else {
            ""
        };
        let _ = write!(
            out,
            r#"<button type="button" class="lc-diff-file-tab{active_cls}" data-testid="diff-file-tab" data-action="select-diff-file" data-arg="{idx}">{} <span class="lc-add">+{}</span> <span class="lc-del">-{}</span></button>"#,
            escape_html(&file.name),
            file.additions,
            file.deletions
        );
    }
    out.push_str("</div>");

    let selected = state
        .diff
        .files
        .get(state.diff.selected_file_index)
        .or_else(|| state.diff.files.first());
    if let Some(file) = selected {
        render_diff_file_lines(out, file);
    }
    out.push_str("</section>");
}

fn render_diff_file_lines(out: &mut String, file: &DecoratedDiffFile) {
    out.push_str(r#"<div class="lc-diff-lines" data-testid="diff-lines">"#);
    for line in &file.lines {
        let kind_cls = match line.kind {
            DiffLineKind::Context => "ctx",
            DiffLineKind::Add => "add",
            DiffLineKind::Delete => "del",
            DiffLineKind::HunkHeader => "hunk",
        };
        let old_ln = line
            .old_line_number
            .map_or_else(String::new, |n| n.to_string());
        let new_ln = line
            .new_line_number
            .map_or_else(String::new, |n| n.to_string());
        let _ = write!(
            out,
            r#"<div class="lc-diff-line lc-diff-{kind_cls}" data-testid="diff-line"><span class="lc-ln">{old_ln}</span><span class="lc-ln">{new_ln}</span><code class="lc-diff-code">"#,
        );
        for run in &line.runs {
            render_diff_run(out, run);
        }
        out.push_str("</code></div>");
    }
    out.push_str("</div>");
}

fn render_diff_run(out: &mut String, run: &AttributedDiffRun) {
    let mut classes = String::from("lc-diff-run");
    if run.font_style.is_bold() {
        classes.push_str(" lc-bold");
    }
    if run.font_style.is_italic() {
        classes.push_str(" lc-italic");
    }
    if run.font_style.is_underline() {
        classes.push_str(" lc-underline");
    }
    if run.font_style.is_strikethrough() {
        classes.push_str(" lc-strike");
    }
    if run.word_highlighted {
        classes.push_str(" lc-word-hl");
    }
    let mut style = String::new();
    if let Some(fg) = run.foreground_color.as_deref().and_then(sanitize_hex_color) {
        let _ = write!(style, "color:{fg};");
    }
    if let Some(bg) = run.background_color.as_deref().and_then(sanitize_hex_color) {
        let _ = write!(style, "background:{bg};");
    }
    let testid_attr = if run.word_highlighted {
        r#" data-testid="diff-word-highlight""#
    } else {
        ""
    };
    let style_attr = if style.is_empty() {
        String::new()
    } else {
        format!(r#" style="{style}""#)
    };
    let _ = write!(
        out,
        r#"<span class="{classes}"{testid_attr}{style_attr} data-u16-start="{}" data-u16-end="{}">{}</span>"#,
        run.start,
        run.end,
        escape_html(&run.text)
    );
}

pub(crate) fn render_terminal_panel(out: &mut String, state: &WebAppState) {
    let snap = state.terminal.active_snapshot();
    out.push_str(r#"<section class="lc-panel lc-terminal-panel" data-testid="terminal-grid">"#);
    let _ = write!(
        out,
        r#"<div class="lc-terminal-bar"><span class="lc-term-title">{}</span><span class="lc-chip">{}×{}</span><span class="lc-chip">scrollback:{}</span><span class="lc-chip">bell:{}</span>"#,
        escape_html(state.terminal.grid.title()),
        snap.cols,
        snap.rows,
        state.terminal.grid.scrollback_len(),
        state.terminal.grid.bell_count()
    );
    if snap.alternate_screen {
        out.push_str(r#"<span class="lc-chip">alt-screen</span>"#);
    }
    let _ = write!(
        out,
        r#"<button type="button" class="lc-btn" data-testid="terminal-export-btn" data-action="terminal-export">{}</button><button type="button" class="lc-btn" data-testid="terminal-reset-btn" data-action="terminal-reset">{}</button></div>"#,
        escape_html(state.locale.web.terminal_export),
        escape_html(state.locale.web.terminal_reset)
    );

    if let Some(ref bytes) = state.terminal.last_exported_lcvt {
        let _ = write!(
            out,
            r#"<div class="lc-term-meta" data-testid="terminal-lcvt-status">LCVT: {} bytes</div>"#,
            bytes.len()
        );
    }
    if let Some(ref err) = state.terminal.snapshot_error {
        let _ = write!(
            out,
            r#"<div class="lc-error" data-testid="terminal-error">{}</div>"#,
            escape_html(err)
        );
    }
    let overrides = snap.palette.override_pairs();
    let _ = write!(
        out,
        r#"<div class="lc-term-palette" data-testid="terminal-palette" data-override-count="{}">"#,
        overrides.len()
    );
    for (idx, rgba) in overrides {
        let hex = format_rgba_hex(rgba);
        let _ = write!(
            out,
            r#"<span class="lc-swatch" data-index="{idx}" style="background:{hex};">[{idx}:{hex}]</span>"#
        );
    }
    out.push_str("</div>");

    render_terminal_rows(out, &snap);

    let _ = write!(
        out,
        r#"<form class="lc-term-paste" data-testid="terminal-paste-form" data-action="terminal-paste"><input type="text" class="lc-input" data-testid="terminal-paste-input" value="{}" placeholder="{}" /></form>"#,
        escape_html(&state.terminal.input_draft),
        escape_html(state.locale.web.terminal_paste_placeholder)
    );
    out.push_str("</section>");
}

fn render_terminal_rows(out: &mut String, snap: &TerminalSnapshot) {
    let cols = usize::from(snap.cols).max(1);
    let fg_hex = format_rgba_hex(snap.foreground);
    let bg_hex = format_rgba_hex(snap.background);
    let _ = write!(
        out,
        r#"<div class="lc-vt-screen" data-testid="terminal-vt-rows" style="color:{fg_hex};background:{bg_hex};">"#
    );
    for (row_idx, row_cells) in snap.cells.chunks(cols).enumerate() {
        out.push_str(r#"<div class="lc-vt-row">"#);
        render_terminal_row_cells(out, snap, row_idx, row_cells);
        out.push_str("</div>");
    }
    out.push_str("</div>");
}

fn render_terminal_row_cells(
    out: &mut String,
    snap: &TerminalSnapshot,
    row_idx: usize,
    cells: &[SnapshotCell],
) {
    let mut idx = 0;
    while idx < cells.len() {
        let cell = &cells[idx];
        let is_cursor = snap.cursor_visible
            && usize::from(snap.cursor_y) == row_idx
            && usize::from(snap.cursor_x) == idx;
        if is_cursor {
            let text = if cell.text.is_empty() {
                " "
            } else {
                &cell.text
            };
            emit_terminal_span(out, cell, true, text);
            idx += 1;
            continue;
        }
        let mut run = String::from(if cell.text.is_empty() {
            " "
        } else {
            &cell.text
        });
        let mut end = idx + 1;
        while end < cells.len() {
            let next = &cells[end];
            let next_cursor = snap.cursor_visible
                && usize::from(snap.cursor_y) == row_idx
                && usize::from(snap.cursor_x) == end;
            if next_cursor
                || next.foreground != cell.foreground
                || next.background != cell.background
                || next.flags != cell.flags
            {
                break;
            }
            run.push_str(if next.text.is_empty() {
                " "
            } else {
                &next.text
            });
            end += 1;
        }
        emit_terminal_span(out, cell, false, &run);
        idx = end;
    }
}

fn emit_terminal_span(out: &mut String, cell: &SnapshotCell, is_cursor: bool, text: &str) {
    let mut classes = String::from("lc-vt-run");
    if cell.flags.contains(GhosttyStyleFlags::BOLD) {
        classes.push_str(" lc-bold");
    }
    if cell.flags.contains(GhosttyStyleFlags::ITALIC) {
        classes.push_str(" lc-italic");
    }
    if cell.flags.contains(GhosttyStyleFlags::UNDERLINE) {
        classes.push_str(" lc-underline");
    }
    if cell.flags.contains(GhosttyStyleFlags::STRIKETHROUGH) {
        classes.push_str(" lc-strike");
    }
    if is_cursor {
        classes.push_str(" lc-vt-cursor");
    }
    let fg = format_rgba_hex(cell.foreground);
    let bg = format_rgba_hex(cell.background);
    let cursor_attr = if is_cursor {
        r#" data-testid="terminal-cursor""#
    } else {
        ""
    };
    let _ = write!(
        out,
        r#"<span class="{classes}"{cursor_attr} style="color:{fg};background:{bg};">{}</span>"#,
        escape_html(text)
    );
}
