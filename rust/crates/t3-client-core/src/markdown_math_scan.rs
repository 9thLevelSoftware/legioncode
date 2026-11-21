//! Inline prose math scanner (`$`, `$$`, `\(`, `\[`) with anti-backtracking bounds.

use crate::markdown_inline_links::{
    RawSpan, is_escaped_at, skip_code_span, try_scan_angle_destination, try_scan_link_destination,
    try_scan_prose_windows_path, try_skip_autolink_url,
};
use crate::markdown_scanner::{MarkdownSpanKind, MathDelimiter};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct FailedScanBounds {
    pub(crate) paren_failed_until: usize,
    pub(crate) bracket_failed_until: usize,
}

pub(crate) fn scan_inline_prose(
    source: &str,
    start: usize,
    end: usize,
    in_table: bool,
    bounds: &mut FailedScanBounds,
    out: &mut Vec<RawSpan>,
) {
    let bytes = source.as_bytes();
    let mut i = start;
    while i < end {
        if bytes[i] >= 0x80 {
            i += source[i..].chars().next().map_or(1, char::len_utf8);
            continue;
        }
        if bytes[i] == b'`' {
            i = skip_code_span(bytes, i, end);
            continue;
        }
        if bytes[i] == b'<'
            && let Some(next_i) = try_scan_angle_destination(source, i, end, in_table, out)
        {
            i = next_i;
            continue;
        }
        if bytes[i] == b']'
            && bytes.get(i + 1) == Some(&b'(')
            && !is_escaped_at(bytes, start, i)
            && let Some(next_i) = try_scan_link_destination(source, i + 2, end, in_table, out)
        {
            i = next_i;
            continue;
        }
        if let Some(url_end) = try_skip_autolink_url(source, i, end) {
            i = url_end;
            continue;
        }
        if let Some(path_end) = try_scan_prose_windows_path(source, i, end, in_table, out) {
            i = path_end;
            continue;
        }
        if bytes[i] == b'$' && !is_escaped_at(bytes, start, i) {
            if bytes.get(i + 1) == Some(&b'$') {
                if let Some((span, next_i)) = scan_inline_double_dollar(source, i, end, in_table) {
                    out.push(span);
                    i = next_i;
                    continue;
                }
                i += 2;
                continue;
            }
            if let Some((span, next_i)) = scan_inline_single_dollar(source, i, end, in_table) {
                out.push(span);
                i = next_i;
                continue;
            }
            i += 1;
            continue;
        }
        if bytes[i] == b'\\' && !is_escaped_at(bytes, start, i) {
            let is_bracket = match bytes.get(i + 1) {
                Some(&b'(') => Some(false),
                Some(&b'[') => Some(true),
                _ => None,
            };
            if let Some(bracket) = is_bracket {
                if let Some((span, next_i)) =
                    scan_tex_inline(source, i, end, bracket, in_table, bounds)
                {
                    out.push(span);
                    i = next_i;
                    continue;
                }
                i += 2;
                continue;
            }
        }
        i += 1;
    }
}

fn scan_inline_single_dollar(
    source: &str,
    start: usize,
    end: usize,
    in_table: bool,
) -> Option<(RawSpan, usize)> {
    let bytes = source.as_bytes();
    let next = *bytes.get(start + 1)?;
    if matches!(next, b' ' | b'\t' | b',' | b'\n' | b'\r') || start + 1 >= end {
        return None;
    }
    let mut i = start + 1;
    let mut slashes = 0_usize;
    while i < end {
        let b = bytes[i];
        if b == b'\n' || b == b'\r' || b == b'`' {
            return None;
        }
        if b == b'/'
            && bytes.get(i.saturating_sub(1)) == Some(&b'/')
            && i >= start + 3
            && bytes[i - 2] == b':'
        {
            return None;
        }
        if b == b'$' && slashes.is_multiple_of(2) {
            if bytes.get(i + 1) == Some(&b'$') {
                return None;
            }
            let prev = bytes[i - 1];
            if matches!(prev, b' ' | b'\t' | b'\n' | b'\r') {
                return None;
            }
            let after = bytes.get(i + 1).copied();
            if after.is_some_and(|ch| ch.is_ascii_digit()) {
                return None;
            }
            if prev == b','
                && let Some(ch) = after
                && !ch.is_ascii_whitespace()
                && !matches!(ch, b'.' | b',' | b';' | b':' | b')' | b']' | b'}')
            {
                return None;
            }
            return Some((
                RawSpan {
                    kind: MarkdownSpanKind::Math {
                        display: false,
                        delimiter: MathDelimiter::SingleDollar,
                        tex: source[start + 1..i].to_owned(),
                    },
                    byte_start: start,
                    byte_end: i + 1,
                    in_table,
                },
                i + 1,
            ));
        }
        slashes = if b == b'\\' { slashes + 1 } else { 0 };
        i += 1;
    }
    None
}

fn scan_inline_double_dollar(
    source: &str,
    start: usize,
    end: usize,
    in_table: bool,
) -> Option<(RawSpan, usize)> {
    let bytes = source.as_bytes();
    if start + 2 >= end || matches!(bytes[start + 2], b'\n' | b'\r') {
        return None;
    }
    let open_ws = matches!(bytes[start + 2], b' ' | b'\t' | b',');
    let mut i = start + 2;
    let mut slashes = 0_usize;
    while i + 1 < end {
        let b = bytes[i];
        if b == b'`' {
            return None;
        }
        if b == b'$' && bytes[i + 1] == b'$' && slashes.is_multiple_of(2) {
            let prev = bytes[i - 1];
            if matches!(prev, b'\n' | b'\r') {
                return None;
            }
            let after = bytes.get(i + 2).copied();
            if after.is_some_and(|ch| ch.is_ascii_digit()) {
                return None;
            }
            if prev == b','
                && !open_ws
                && let Some(ch) = after
                && !ch.is_ascii_whitespace()
                && !matches!(ch, b'.' | b',' | b';' | b':' | b')' | b']' | b'}')
            {
                return None;
            }
            let close_ws = matches!(prev, b' ' | b'\t') || (prev == b',' && open_ws);
            if open_ws != close_ws {
                return None;
            }
            return Some((
                RawSpan {
                    kind: MarkdownSpanKind::Math {
                        display: true,
                        delimiter: MathDelimiter::DoubleDollar,
                        tex: source[start + 2..i].to_owned(),
                    },
                    byte_start: start,
                    byte_end: i + 2,
                    in_table,
                },
                i + 2,
            ));
        }
        slashes = if b == b'\\' { slashes + 1 } else { 0 };
        i += 1;
    }
    None
}

fn scan_tex_inline(
    source: &str,
    start: usize,
    end: usize,
    is_bracket: bool,
    in_table: bool,
    bounds: &mut FailedScanBounds,
) -> Option<(RawSpan, usize)> {
    let failed_until = if is_bracket {
        &mut bounds.bracket_failed_until
    } else {
        &mut bounds.paren_failed_until
    };
    if *failed_until > start {
        return None;
    }
    let bytes = source.as_bytes();
    let close_char = if is_bracket { b']' } else { b')' };
    let mut i = start + 2;
    let mut substantive = false;
    while i < end {
        let b = bytes[i];
        if b == b'`' {
            *failed_until = i;
            return None;
        }
        if b == close_char && is_escaped_at(bytes, start + 2, i) {
            if is_bracket && !substantive {
                return None;
            }
            let tex = source[start + 2..i - 1].to_owned();
            let delimiter = if is_bracket {
                MathDelimiter::Bracket
            } else {
                MathDelimiter::Paren
            };
            return Some((
                RawSpan {
                    kind: MarkdownSpanKind::Math {
                        display: is_bracket,
                        delimiter,
                        tex,
                    },
                    byte_start: start,
                    byte_end: i + 1,
                    in_table,
                },
                i + 1,
            ));
        }
        if !matches!(b, b'\\' | b' ' | b',' | b'\n' | b'\r' | b'0'..=b'9') {
            substantive = true;
        }
        i += 1;
    }
    *failed_until = end;
    None
}
