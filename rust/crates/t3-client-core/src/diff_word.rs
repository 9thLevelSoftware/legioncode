//! Header parsing and `similar` intra-line word-diff helpers for unified diffs.

use similar::{ChangeTag, TextDiff};
use t3_shared::utf16::utf8_len_to_utf16;

use crate::diff_parser::{DiffHunkMeta, decode_diff_file_name};
use crate::diff_soa::Utf16Span;

pub(crate) struct InFlightHunk {
    pub(crate) meta: DiffHunkMeta,
    pub(crate) old_cursor: u32,
    pub(crate) new_cursor: u32,
    pub(crate) old_remaining: u32,
    pub(crate) new_remaining: u32,
}

pub(crate) fn parse_git_diff_header_paths(rest: &str) -> (Option<String>, Option<String>) {
    let trimmed = rest.trim_end_matches(['\t', '\r', '\n']);
    if trimmed.starts_with('"')
        && let Some((first, tail)) = split_quoted_token(trimmed)
        && let Some(second_raw) = tail.strip_prefix(' ')
    {
        return (
            Some(decode_diff_file_name(first, true)),
            Some(decode_diff_file_name(second_raw, true)),
        );
    }
    if let Some(pos) = trimmed.find(" b/") {
        return (
            Some(decode_diff_file_name(&trimmed[..pos], true)),
            Some(decode_diff_file_name(&trimmed[pos + 1..], true)),
        );
    }
    (None, None)
}

fn split_quoted_token(s: &str) -> Option<(&str, &str)> {
    let bytes = s.as_bytes();
    if *bytes.first()? != b'"' {
        return None;
    }
    let mut i = 1;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += 2;
            continue;
        }
        if bytes[i] == b'"' {
            return Some((&s[..=i], &s[i + 1..]));
        }
        i += 1;
    }
    None
}

pub(crate) fn parse_hunk_header(line: &str, line_start: usize) -> Option<InFlightHunk> {
    let after_prefix = line.strip_prefix("@@ ")?;
    let end_marker = after_prefix.find(" @@")?;
    let ranges = &after_prefix[..end_marker];
    let context_raw = &after_prefix[end_marker + 3..];
    let context = context_raw
        .strip_prefix(' ')
        .map(str::to_owned)
        .filter(|s| !s.is_empty());
    let mut parts = ranges.split_whitespace();
    let old_part = parts.next()?.strip_prefix('-')?;
    let new_part = parts.next()?.strip_prefix('+')?;
    let (old_start, old_count) = parse_range_pair(old_part)?;
    let (new_start, new_count) = parse_range_pair(new_part)?;
    Some(InFlightHunk {
        meta: DiffHunkMeta {
            old_start,
            old_count,
            new_start,
            new_count,
            context,
            line_start_index: line_start,
            line_count: 0,
            addition_lines: 0,
            deletion_lines: 0,
            no_eof_cr_additions: false,
            no_eof_cr_deletions: false,
        },
        old_cursor: old_start,
        new_cursor: new_start,
        old_remaining: old_count,
        new_remaining: new_count,
    })
}

fn parse_range_pair(part: &str) -> Option<(u32, u32)> {
    if let Some((start, count)) = part.split_once(',') {
        Some((start.parse().ok()?, count.parse().ok()?))
    } else {
        Some((part.parse().ok()?, 1))
    }
}

pub(crate) fn compute_word_diff_spans(
    old_line: &str,
    new_line: &str,
) -> (Vec<Utf16Span>, Vec<Utf16Span>) {
    let diff = TextDiff::from_words(old_line, new_line);
    let mut old_spans = Vec::new();
    let mut new_spans = Vec::new();
    let (mut old_u16, mut new_u16) = (0_usize, 0_usize);
    for change in diff.iter_all_changes() {
        let u16_len = utf8_len_to_utf16(change.value());
        match change.tag() {
            ChangeTag::Equal => {
                old_u16 += u16_len;
                new_u16 += u16_len;
            }
            ChangeTag::Delete => {
                push_merged_span(&mut old_spans, old_u16, old_u16 + u16_len);
                old_u16 += u16_len;
            }
            ChangeTag::Insert => {
                push_merged_span(&mut new_spans, new_u16, new_u16 + u16_len);
                new_u16 += u16_len;
            }
        }
    }
    (old_spans, new_spans)
}

fn push_merged_span(spans: &mut Vec<Utf16Span>, start: usize, end: usize) {
    if start >= end {
        return;
    }
    let start_u32 = u32::try_from(start).unwrap_or(u32::MAX);
    let end_u32 = u32::try_from(end).unwrap_or(u32::MAX);
    if let Some(last) = spans.last_mut()
        && last.end == start_u32
    {
        last.end = end_u32;
        return;
    }
    spans.push(Utf16Span {
        start: start_u32,
        end: end_u32,
    });
}
