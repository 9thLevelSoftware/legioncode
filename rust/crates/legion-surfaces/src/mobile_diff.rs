//! Unified mobile review-diff attributed run decoration (`BUG-22`).
//!
//! Replaces the divergent iOS (`LegionReviewDiffView.swift` / `ReviewDiffCodeLayout.swift`) and
//! Android (`ReviewDiffCodeLayout.kt`) token attribute builders with one pure-Rust run splitter
//! and coalescer that preserves `FontStyleFlags` (`BOLD`, `ITALIC`, `UNDERLINE`, `STRIKETHROUGH`),
//! foreground/background colors, ligature settings, note flags, and arbitrary non-color attributes
//! across intra-line word-diff highlight boundaries.

use legion_client_core::diff_markdown::{DiffLineArrays, DiffLineKind, Utf16Span};

pub use crate::mobile_diff_types::{AttributedDiffRun, FontStyleFlags, SyntaxTokenInput};

/// Converts sequential [`SyntaxTokenInput`] items into contiguous UTF-16 [`AttributedDiffRun`]s.
pub fn build_syntax_runs_from_tokens(tokens: &[SyntaxTokenInput]) -> Vec<AttributedDiffRun> {
    let mut runs = Vec::with_capacity(tokens.len());
    let mut cursor = 0_u32;
    for token in tokens {
        if token.content.is_empty() {
            continue;
        }
        let u16_len = u32::try_from(token.content.encode_utf16().count()).unwrap_or(u32::MAX);
        let end = cursor.saturating_add(u16_len);
        runs.push(AttributedDiffRun {
            start: cursor,
            end,
            text: token.content.clone(),
            foreground_color: token.color.clone(),
            background_color: None,
            font_style: token.font_style,
            word_highlighted: false,
            ligatures_enabled: token.ligatures_enabled,
            is_note: false,
            attributes: token.attributes.clone(),
        });
        cursor = end;
    }
    runs
}

/// Splits and merges `syntax_runs` against `word_highlight_ranges` without stripping
/// `font_style` flags or non-color attributes (`BUG-22`).
pub fn decorate_diff_runs(
    syntax_runs: &[AttributedDiffRun],
    word_highlight_ranges: &[Utf16Span],
) -> Vec<AttributedDiffRun> {
    let highlights = merge_highlight_ranges(word_highlight_ranges);
    let mut split_runs = Vec::with_capacity(syntax_runs.len() + highlights.len());

    for run in syntax_runs {
        if run.start >= run.end {
            continue;
        }
        let mut cuts = vec![run.start, run.end];
        for span in &highlights {
            if span.end <= run.start || span.start >= run.end {
                continue;
            }
            if span.start > run.start && span.start < run.end {
                cuts.push(span.start);
            }
            if span.end > run.start && span.end < run.end {
                cuts.push(span.end);
            }
        }
        cuts.sort_unstable();
        cuts.dedup();

        for pair in cuts.windows(2) {
            let seg_start = pair[0];
            let seg_end = pair[1];
            if seg_start >= seg_end {
                continue;
            }
            let overlaps = highlights
                .iter()
                .any(|span| span.start < seg_end && span.end > seg_start);
            let seg_text =
                slice_utf16_relative(&run.text, seg_start - run.start, seg_end - run.start);
            split_runs.push(AttributedDiffRun {
                start: seg_start,
                end: seg_end,
                text: seg_text,
                foreground_color: run.foreground_color.clone(),
                background_color: run.background_color.clone(),
                font_style: run.font_style,
                word_highlighted: run.word_highlighted || overlaps,
                ligatures_enabled: run.ligatures_enabled,
                is_note: run.is_note,
                attributes: run.attributes.clone(),
            });
        }
    }

    coalesce_runs(split_runs)
}

/// Applies [`decorate_diff_runs`] and fills `background_color` on non-note highlighted segments
/// when no explicit background color is already set.
pub fn decorate_diff_runs_with_background(
    syntax_runs: &[AttributedDiffRun],
    word_highlight_ranges: &[Utf16Span],
    highlight_background: &str,
) -> Vec<AttributedDiffRun> {
    let mut runs = decorate_diff_runs(syntax_runs, word_highlight_ranges);
    for run in &mut runs {
        if run.word_highlighted && !run.is_note && run.background_color.is_none() {
            run.background_color = Some(highlight_background.to_owned());
        }
    }
    runs
}

/// Bridges a [`DiffLineArrays`] line and its `SoA` syntax tokens into decorated [`AttributedDiffRun`]s.
pub fn decorate_soa_line(
    arrays: &DiffLineArrays,
    line_index: usize,
) -> Option<Vec<AttributedDiffRun>> {
    let line = arrays.line(line_index)?;
    let tokens = arrays.line_tokens(line_index);
    let syntax_runs = if tokens.is_empty() {
        let len = u32::try_from(line.text.encode_utf16().count()).unwrap_or(u32::MAX);
        if len == 0 {
            Vec::new()
        } else {
            vec![AttributedDiffRun::new(0, len, line.text)]
        }
    } else {
        let mut cursor = 0_u32;
        tokens
            .into_iter()
            .filter(|tok| !tok.text.is_empty())
            .map(|tok| {
                let len = u32::try_from(tok.text.encode_utf16().count()).unwrap_or(u32::MAX);
                let end = cursor.saturating_add(len);
                let mut run = AttributedDiffRun::new(cursor, end, tok.text)
                    .with_font_style(FontStyleFlags::from_bits_truncate(tok.font_style));
                if let Some(color) = tok.color {
                    run.foreground_color = Some(format_packed_hex_color(color));
                }
                cursor = end;
                run
            })
            .collect()
    };

    let highlights = match line.kind {
        DiffLineKind::Add | DiffLineKind::Delete => line.word_diffs,
        DiffLineKind::Context | DiffLineKind::HunkHeader => &[],
    };
    Some(decorate_diff_runs(&syntax_runs, highlights))
}

fn merge_highlight_ranges(ranges: &[Utf16Span]) -> Vec<Utf16Span> {
    let mut valid: Vec<Utf16Span> = ranges.iter().copied().filter(|r| r.start < r.end).collect();
    valid.sort_unstable_by_key(|r| (r.start, r.end));
    let mut merged: Vec<Utf16Span> = Vec::with_capacity(valid.len());
    for span in valid {
        if let Some(last) = merged.last_mut()
            && span.start <= last.end
        {
            last.end = last.end.max(span.end);
            continue;
        }
        merged.push(span);
    }
    merged
}

fn coalesce_runs(runs: Vec<AttributedDiffRun>) -> Vec<AttributedDiffRun> {
    let mut merged: Vec<AttributedDiffRun> = Vec::with_capacity(runs.len());
    for run in runs {
        if let Some(last) = merged.last_mut()
            && last.end == run.start
            && last.has_same_style(&run)
        {
            last.end = run.end;
            last.text.push_str(&run.text);
            continue;
        }
        merged.push(run);
    }
    merged
}

fn slice_utf16_relative(text: &str, u16_start: u32, u16_end: u32) -> String {
    if text.is_empty() || u16_start >= u16_end {
        return String::new();
    }
    let start_byte = utf16_offset_to_byte_floor(text, u16_start as usize);
    let end_byte = utf16_offset_to_byte_floor(text, u16_end as usize);
    if start_byte >= end_byte {
        return String::new();
    }
    text.get(start_byte..end_byte).unwrap_or("").to_owned()
}

fn utf16_offset_to_byte_floor(text: &str, target_u16: usize) -> usize {
    let mut u16_pos = 0_usize;
    for (byte_idx, ch) in text.char_indices() {
        let ch_u16 = ch.len_utf16();
        if u16_pos + ch_u16 > target_u16 {
            return byte_idx;
        }
        u16_pos += ch_u16;
    }
    text.len()
}

fn format_packed_hex_color(color: u32) -> String {
    if color <= 0x00FF_FFFF {
        format!("#{color:06x}")
    } else {
        format!("#{color:08x}")
    }
}
