//! Block-level Markdown partitioning (fences, indented code, flow math, GFM tables).

use crate::markdown_inline_links::RawSpan;
use crate::markdown_math_scan::{FailedScanBounds, scan_inline_prose};
use crate::markdown_scanner::{MarkdownSpanKind, MathDelimiter};

pub(crate) struct LineSlice {
    pub(crate) start: usize,
    pub(crate) content_end: usize,
}

#[derive(Clone, Copy)]
pub(crate) struct FenceBlockSpec {
    pub(crate) open_idx: usize,
    pub(crate) fence_ch: u8,
    pub(crate) fence_len: usize,
    pub(crate) is_math: bool,
    pub(crate) bq_depth: usize,
}

pub(crate) fn split_source_lines(source: &str) -> Vec<LineSlice> {
    let bytes = source.as_bytes();
    let mut lines = Vec::new();
    let mut start = 0_usize;
    let mut i = 0_usize;
    while i < bytes.len() {
        if bytes[i] == b'\r' {
            lines.push(LineSlice {
                start,
                content_end: i,
            });
            i += if bytes.get(i + 1) == Some(&b'\n') {
                2
            } else {
                1
            };
            start = i;
        } else if bytes[i] == b'\n' {
            lines.push(LineSlice {
                start,
                content_end: i,
            });
            i += 1;
            start = i;
        } else {
            i += 1;
        }
    }
    if start < bytes.len() {
        lines.push(LineSlice {
            start,
            content_end: bytes.len(),
        });
    }
    lines
}

pub(crate) fn strip_blockquote_prefix(line: &str) -> (usize, &str) {
    let mut rest = line;
    let mut depth = 0_usize;
    loop {
        let after_spaces = trim_up_to_3_spaces(rest);
        if let Some(after_gt) = after_spaces.strip_prefix('>') {
            depth += 1;
            rest = after_gt.strip_prefix(' ').unwrap_or(after_gt);
        } else {
            return (depth, rest);
        }
    }
}

fn trim_up_to_3_spaces(s: &str) -> &str {
    let b = s.as_bytes();
    let mut count = 0_usize;
    while count < 3 && b.get(count) == Some(&b' ') {
        count += 1;
    }
    &s[count..]
}

pub(crate) fn is_indented_code_line(s: &str) -> bool {
    s.starts_with("    ") || s.starts_with('\t')
}

pub(crate) fn parse_fence_opener(after_bq: &str) -> Option<(u8, usize, &str)> {
    let trimmed = trim_up_to_3_spaces(after_bq);
    let ch = *trimmed.as_bytes().first()?;
    if ch != b'`' && ch != b'~' {
        return None;
    }
    let len = trimmed.bytes().take_while(|&b| b == ch).count();
    if len < 3 {
        return None;
    }
    let info = trimmed[len..].trim();
    if ch == b'`' && info.contains('`') {
        return None;
    }
    Some((ch, len, info))
}

pub(crate) fn is_math_info(info: &str) -> bool {
    info == "math"
        || info
            .strip_prefix("math")
            .is_some_and(|rest| rest.starts_with([' ', '\t']))
}

pub(crate) fn scan_fence_block(
    lines: &[LineSlice],
    source: &str,
    spec: FenceBlockSpec,
    out: &mut Vec<RawSpan>,
) -> usize {
    let mut close_idx = None;
    for (offset, line) in lines[spec.open_idx + 1..].iter().enumerate() {
        let (depth, after_bq) = strip_blockquote_prefix(&source[line.start..line.content_end]);
        if depth < spec.bq_depth && !after_bq.trim().is_empty() {
            continue;
        }
        let trimmed = trim_up_to_3_spaces(after_bq);
        let run = trimmed.bytes().take_while(|&b| b == spec.fence_ch).count();
        if run >= spec.fence_len && trimmed[run..].trim().is_empty() {
            close_idx = Some(spec.open_idx + 1 + offset);
            break;
        }
    }
    if let Some(end_line) = close_idx {
        if spec.is_math {
            let mut parts = Vec::with_capacity(end_line.saturating_sub(spec.open_idx + 1));
            for line in &lines[spec.open_idx + 1..end_line] {
                let (_, after) = strip_blockquote_prefix_up_to(
                    &source[line.start..line.content_end],
                    spec.bq_depth,
                );
                parts.push(after);
            }
            out.push(RawSpan {
                kind: MarkdownSpanKind::Math {
                    display: true,
                    delimiter: MathDelimiter::MathFence,
                    tex: parts.join("\n"),
                },
                byte_start: lines[spec.open_idx].start,
                byte_end: lines[end_line].content_end,
                in_table: false,
            });
        }
        end_line + 1
    } else {
        lines.len()
    }
}

pub(crate) fn skip_indented_block(lines: &[LineSlice], source: &str, mut idx: usize) -> usize {
    while idx < lines.len() {
        let next_text = &source[lines[idx].start..lines[idx].content_end];
        let (_, next_after_bq) = strip_blockquote_prefix(next_text);
        if next_after_bq.trim().is_empty() || is_indented_code_line(next_after_bq) {
            idx += 1;
        } else {
            break;
        }
    }
    idx
}

fn strip_blockquote_prefix_up_to(line: &str, max_depth: usize) -> (usize, &str) {
    let mut rest = line;
    let mut depth = 0_usize;
    while depth < max_depth {
        let after_spaces = trim_up_to_3_spaces(rest);
        if let Some(after_gt) = after_spaces.strip_prefix('>') {
            depth += 1;
            rest = after_gt.strip_prefix(' ').unwrap_or(after_gt);
        } else {
            break;
        }
    }
    (depth, rest)
}

pub(crate) fn is_flow_math_opener(after_bq: &str, marker: &str) -> bool {
    trim_up_to_3_spaces(after_bq)
        .strip_prefix(marker)
        .is_some_and(|rest| rest.trim_matches([' ', '\t']).is_empty())
}

pub(crate) fn scan_flow_dollar_block(
    lines: &[LineSlice],
    source: &str,
    open_idx: usize,
    bq_depth: usize,
) -> Option<(usize, String)> {
    let mut body = Vec::new();
    for (offset, line) in lines[open_idx + 1..].iter().enumerate() {
        let raw = &source[line.start..line.content_end];
        let (depth, after_bq) = strip_blockquote_prefix_up_to(raw, bq_depth);
        if bq_depth > 0 && (depth < bq_depth || after_bq.trim().is_empty()) {
            return None;
        }
        if is_flow_math_opener(after_bq, "$$") {
            return Some((open_idx + 1 + offset, body.join("\n")));
        }
        let trimmed_end = after_bq.trim_end_matches([' ', '\t']);
        if let Some(prefix) = trimmed_end.strip_suffix("$$")
            && prefix
                .bytes()
                .rev()
                .take_while(|&b| b == b'\\')
                .count()
                .is_multiple_of(2)
        {
            if !prefix.is_empty() {
                body.push(prefix);
            }
            return Some((open_idx + 1 + offset, body.join("\n")));
        }
        body.push(after_bq);
    }
    None
}

pub(crate) fn scan_flow_bracket_block(
    lines: &[LineSlice],
    source: &str,
    open_idx: usize,
    bq_depth: usize,
) -> Result<(usize, String), usize> {
    let mut body = Vec::new();
    for (offset, line) in lines[open_idx + 1..].iter().enumerate() {
        let raw = &source[line.start..line.content_end];
        let (depth, after_bq) = strip_blockquote_prefix_up_to(raw, bq_depth);
        if bq_depth > 0 && (depth < bq_depth || after_bq.trim().is_empty()) {
            return Err(line.start);
        }
        if is_flow_math_opener(after_bq, "\\]") {
            return Ok((open_idx + 1 + offset, body.join("\n")));
        }
        body.push(after_bq);
    }
    Err(source.len())
}

pub(crate) fn is_table_start(
    lines: &[LineSlice],
    source: &str,
    idx: usize,
    text: &str,
    bq_depth: usize,
) -> bool {
    if idx + 1 >= lines.len() || !text.contains('|') {
        return false;
    }
    let delim = &source[lines[idx + 1].start..lines[idx + 1].content_end];
    let (depth, after_bq) = strip_blockquote_prefix(delim);
    if depth != bq_depth {
        return false;
    }
    let trimmed = after_bq.trim();
    !trimmed.is_empty()
        && trimmed.contains('-')
        && (trimmed.contains('|') || trimmed.contains(':'))
        && trimmed
            .bytes()
            .all(|b| matches!(b, b' ' | b'\t' | b'|' | b':' | b'-'))
}

pub(crate) fn scan_table_block(
    lines: &[LineSlice],
    source: &str,
    start_idx: usize,
    bq_depth: usize,
    bounds: &mut FailedScanBounds,
    out: &mut Vec<RawSpan>,
) -> usize {
    scan_inline_prose(
        source,
        lines[start_idx].start,
        lines[start_idx].content_end,
        true,
        bounds,
        out,
    );
    let mut idx = start_idx + 2;
    while idx < lines.len() {
        let row_text = &source[lines[idx].start..lines[idx].content_end];
        let (row_bq, row_after) = strip_blockquote_prefix(row_text);
        if row_bq != bq_depth || row_after.trim().is_empty() || !row_after.contains('|') {
            break;
        }
        scan_inline_prose(
            source,
            lines[idx].start,
            lines[idx].content_end,
            true,
            bounds,
            out,
        );
        idx += 1;
    }
    idx
}

pub(crate) fn scan_paragraph_block(
    lines: &[LineSlice],
    source: &str,
    start_idx: usize,
    bq_depth: usize,
    bounds: &mut FailedScanBounds,
    out: &mut Vec<RawSpan>,
) -> usize {
    let para_start = lines[start_idx].start;
    let mut para_end = lines[start_idx].content_end;
    let mut next_idx = start_idx + 1;
    while next_idx < lines.len() {
        let next_line = &lines[next_idx];
        let next_text = &source[next_line.start..next_line.content_end];
        let (next_bq, next_after) = strip_blockquote_prefix(next_text);
        if next_after.trim().is_empty()
            || next_bq != bq_depth
            || parse_fence_opener(next_after).is_some()
            || is_flow_math_opener(next_after, "$$")
            || is_flow_math_opener(next_after, "\\[")
            || is_table_start(lines, source, next_idx, next_text, next_bq)
        {
            break;
        }
        para_end = next_line.content_end;
        next_idx += 1;
    }
    scan_inline_prose(source, para_start, para_end, false, bounds, out);
    next_idx
}
