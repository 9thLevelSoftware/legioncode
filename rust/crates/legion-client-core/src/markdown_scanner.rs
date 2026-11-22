//! Math- and Windows-path-safe Markdown span scanner.
//!
//! Mirrors `packages/shared/src/{markdownWindowsPaths.ts,markdownMath.ts,path.ts}` and
//! `patches/micromark-extension-llm-math@3.1.0.patch`.

use legion_shared::utf16::utf8_len_to_utf16;
use serde::{Deserialize, Serialize};

use crate::markdown_blocks::{
    FenceBlockSpec, is_flow_math_opener, is_indented_code_line, is_math_info, is_table_start,
    parse_fence_opener, scan_fence_block, scan_flow_bracket_block, scan_flow_dollar_block,
    scan_paragraph_block, scan_table_block, skip_indented_block, split_source_lines,
    strip_blockquote_prefix,
};
use crate::markdown_inline_links::RawSpan;
use crate::markdown_math_scan::FailedScanBounds;

/// Math delimiter style matched by [`scan_markdown_spans`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MathDelimiter {
    /// Inline `$...$`.
    SingleDollar,
    /// Display `$$...$$`.
    DoubleDollar,
    /// Inline `\(...\)`.
    Paren,
    /// Display `\[...\]`.
    Bracket,
    /// Fenced ```` ```math ```` or `~~~math` code block.
    MathFence,
}

/// Semantic span classification emitted by [`scan_markdown_spans`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum MarkdownSpanKind {
    /// Closed math formula with extracted TeX body.
    Math {
        /// `true` for display math (`$$...$$`, `\[...\]`, ```` ```math ````).
        display: bool,
        /// Opening/closing delimiter family.
        delimiter: MathDelimiter,
        /// Unwrapped TeX expression payload.
        tex: String,
    },
    /// Literal Windows drive or UNC path whose backslashes must not be treated as escapes.
    WindowsPath {
        /// Authored Windows path text.
        path: String,
        /// `true` when extracted from a Markdown link/image destination (`](...)` / `<...>`).
        in_destination: bool,
    },
}

/// Located span returned by [`scan_markdown_spans`] with UTF-8 byte and UTF-16 code-unit offsets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarkdownSpan {
    /// Span classification and payload.
    pub kind: MarkdownSpanKind,
    /// Inclusive UTF-8 byte offset into the source string.
    pub byte_start: usize,
    /// Exclusive UTF-8 byte offset into the source string.
    pub byte_end: usize,
    /// Inclusive UTF-16 code-unit offset into the source string.
    pub utf16_start: usize,
    /// Exclusive UTF-16 code-unit offset into the source string.
    pub utf16_end: usize,
    /// Whether this span sits inside a GFM table cell.
    pub in_table: bool,
}

/// Returns `true` when `value` begins with a Windows drive designator (`C:`, `C:\...`, `C:/...`).
pub fn is_windows_drive_path(value: &str) -> bool {
    let b = value.as_bytes();
    b.len() >= 2
        && b[0].is_ascii_alphabetic()
        && b[1] == b':'
        && (b.len() == 2 || matches!(b[2], b'/' | b'\\'))
}

/// Returns `true` when `value` begins with a UNC prefix (`\\`).
pub fn is_unc_path(value: &str) -> bool {
    value.starts_with("\\\\")
}

/// Returns `true` for Windows drive or UNC absolute paths (`packages/shared/src/path.ts`).
pub fn is_windows_absolute_path(value: &str) -> bool {
    is_unc_path(value) || is_windows_drive_path(value)
}

/// Preserves backslashes in Windows path link destinations while unescaping standard `CommonMark`.
pub fn keep_windows_path_destination(authored: &str) -> String {
    if is_windows_absolute_path(authored) {
        return authored.to_owned();
    }
    let bytes = authored.as_bytes();
    let mut out = String::with_capacity(authored.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && let Some(&next) = bytes.get(i + 1)
            && next.is_ascii_punctuation()
        {
            out.push(char::from(next));
            i += 2;
            continue;
        }
        if let Some(ch) = authored[i..].chars().next() {
            out.push(ch);
            i += ch.len_utf8();
        } else {
            break;
        }
    }
    out
}

/// Fast linear prefilter returning `false` when `text` cannot possibly contain Markdown math.
pub fn may_contain_math(text: &str) -> bool {
    if text.contains("\\(") || text.contains("\\[") || contains_math_fence_hint(text) {
        return true;
    }
    let bytes = text.as_bytes();
    let mut opened = false;
    for (idx, &b) in bytes.iter().enumerate() {
        if b == b'\n' || b == b'\r' {
            opened = false;
        } else if b == b'$' {
            if opened && bytes.get(idx.saturating_sub(1)) != Some(&b' ') {
                return true;
            }
            opened = true;
        }
    }
    false
}

fn contains_math_fence_hint(text: &str) -> bool {
    for line in text.lines() {
        let trimmed = line.trim_start_matches([' ', '\t', '>']);
        if (trimmed.starts_with("```") || trimmed.starts_with("~~~"))
            && let Some(info) = trimmed
                .get(3..)
                .map(|s| s.trim_start_matches(['`', '~', ' ', '\t']))
            && info.starts_with("math")
            && info
                .as_bytes()
                .get(4)
                .is_none_or(|b| !b.is_ascii_alphanumeric() && *b != b'_')
        {
            return true;
        }
    }
    false
}

/// Scans `source` for closed math spans and Windows paths while respecting code blocks and tables.
pub fn scan_markdown_spans(source: &str) -> Vec<MarkdownSpan> {
    let lines = split_source_lines(source);
    let mut raw_spans = Vec::new();
    let mut inline_bounds = FailedScanBounds::default();
    let mut flow_bracket_failed_until = 0_usize;
    let mut idx = 0_usize;
    let mut prev_blank = true;

    while idx < lines.len() {
        let line = &lines[idx];
        let text = &source[line.start..line.content_end];
        let (bq_depth, after_bq) = strip_blockquote_prefix(text);

        if after_bq.trim().is_empty() {
            prev_blank = true;
            idx += 1;
            continue;
        }
        if let Some((fence_ch, fence_len, info)) = parse_fence_opener(after_bq) {
            let spec = FenceBlockSpec {
                open_idx: idx,
                fence_ch,
                fence_len,
                is_math: is_math_info(info),
                bq_depth,
            };
            idx = scan_fence_block(&lines, source, spec, &mut raw_spans);
            prev_blank = false;
            continue;
        }
        if prev_blank && is_indented_code_line(after_bq) {
            idx = skip_indented_block(&lines, source, idx + 1);
            prev_blank = false;
            continue;
        }
        if is_flow_math_opener(after_bq, "$$")
            && let Some((end_line, tex)) = scan_flow_dollar_block(&lines, source, idx, bq_depth)
        {
            raw_spans.push(RawSpan {
                kind: MarkdownSpanKind::Math {
                    display: true,
                    delimiter: MathDelimiter::DoubleDollar,
                    tex,
                },
                byte_start: line.start,
                byte_end: lines[end_line].content_end,
                in_table: false,
            });
            idx = end_line + 1;
            prev_blank = false;
            continue;
        }
        if is_flow_math_opener(after_bq, "\\[") {
            if line.start < flow_bracket_failed_until {
                idx += 1;
                prev_blank = false;
                continue;
            }
            match scan_flow_bracket_block(&lines, source, idx, bq_depth) {
                Ok((end_line, tex)) => {
                    raw_spans.push(RawSpan {
                        kind: MarkdownSpanKind::Math {
                            display: true,
                            delimiter: MathDelimiter::Bracket,
                            tex,
                        },
                        byte_start: line.start,
                        byte_end: lines[end_line].content_end,
                        in_table: false,
                    });
                    idx = end_line + 1;
                    prev_blank = false;
                    continue;
                }
                Err(stop_byte) => {
                    flow_bracket_failed_until = flow_bracket_failed_until.max(stop_byte);
                }
            }
        }
        if is_table_start(&lines, source, idx, text, bq_depth) {
            idx = scan_table_block(
                &lines,
                source,
                idx,
                bq_depth,
                &mut inline_bounds,
                &mut raw_spans,
            );
        } else {
            idx = scan_paragraph_block(
                &lines,
                source,
                idx,
                bq_depth,
                &mut inline_bounds,
                &mut raw_spans,
            );
        }
        prev_blank = false;
    }
    attach_utf16_offsets(source, raw_spans)
}

fn attach_utf16_offsets(source: &str, raw_spans: Vec<RawSpan>) -> Vec<MarkdownSpan> {
    let mut cursor_byte = 0_usize;
    let mut cursor_u16 = 0_usize;
    let mut out = Vec::with_capacity(raw_spans.len());
    for raw in raw_spans {
        if raw.byte_start < cursor_byte
            || raw.byte_end < raw.byte_start
            || raw.byte_end > source.len()
        {
            continue;
        }
        cursor_u16 += utf8_len_to_utf16(&source[cursor_byte..raw.byte_start]);
        let utf16_start = cursor_u16;
        cursor_u16 += utf8_len_to_utf16(&source[raw.byte_start..raw.byte_end]);
        let utf16_end = cursor_u16;
        cursor_byte = raw.byte_end;
        out.push(MarkdownSpan {
            kind: raw.kind,
            byte_start: raw.byte_start,
            byte_end: raw.byte_end,
            utf16_start,
            utf16_end,
            in_table: raw.in_table,
        });
    }
    out
}
