//! `XSS`-safe `HTML5` renderer for the `Ctrl+K` command palette modal.

use std::fmt::Write as _;

use legion_shared::utf16::{utf8_len_to_utf16, utf16_slice_safe};

use crate::state::WebAppState;
use crate::state_types::{CommandPaletteMatch, PaletteSearchRange};
use crate::view::escape_html;

pub(crate) fn render_command_palette_modal(out: &mut String, state: &WebAppState) {
    if !state.command_palette.open {
        return;
    }
    out.push_str(
        r#"<div class="lc-palette-backdrop" data-testid="command-palette-modal" role="dialog" aria-modal="true"><div class="lc-palette-card">"#,
    );
    let _ = write!(
        out,
        r#"<input type="search" class="lc-palette-input" data-testid="command-palette-input" data-action="palette-input" value="{}" placeholder="{}" />"#,
        escape_html(&state.command_palette.query),
        escape_html(state.locale.web.palette_placeholder)
    );
    out.push_str(r#"<ul class="lc-palette-results" data-testid="command-palette-results">"#);
    if state.command_palette.results.is_empty() {
        let _ = write!(
            out,
            r#"<li class="lc-palette-empty">{}</li>"#,
            escape_html(state.locale.web.palette_empty)
        );
    } else {
        for (idx, item) in state.command_palette.results.iter().enumerate() {
            render_palette_item(out, idx, idx == state.command_palette.selected_index, item);
        }
    }
    out.push_str("</ul></div></div>");
}

fn render_palette_item(out: &mut String, idx: usize, selected: bool, item: &CommandPaletteMatch) {
    let active_cls = if selected { " is-selected" } else { "" };
    let _ = write!(
        out,
        r#"<li class="lc-palette-item{active_cls}" data-testid="command-palette-item" data-action="palette-exec" data-arg="{idx}"><span class="lc-palette-title">"#,
    );
    render_highlighted_title(out, &item.title, &item.ranges);
    let _ = write!(
        out,
        r#"</span><span class="lc-palette-sub">{}</span></li>"#,
        escape_html(&item.subtitle)
    );
}

fn render_highlighted_title(out: &mut String, title: &str, ranges: &[PaletteSearchRange]) {
    let total_u16 = utf8_len_to_utf16(title);
    let mut cursor = 0_usize;
    for range in ranges {
        let start = range.start.min(total_u16);
        let end = range.end.min(total_u16);
        if end <= cursor || start >= end {
            continue;
        }
        if start > cursor {
            out.push_str(&escape_html(utf16_slice_safe(title, cursor, start)));
        }
        let slice = utf16_slice_safe(title, start, end);
        let _ = write!(
            out,
            r#"<mark class="lc-palette-mark">{}</mark>"#,
            escape_html(slice)
        );
        cursor = end;
    }
    if cursor < total_u16 {
        out.push_str(&escape_html(utf16_slice_safe(title, cursor, total_u16)));
    }
}
