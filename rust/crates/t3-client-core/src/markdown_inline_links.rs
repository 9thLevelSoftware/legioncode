//! Code-span, link-destination, autolink, and prose Windows-path inline helpers.

use crate::markdown_scanner::{
    MarkdownSpanKind, is_unc_path, is_windows_absolute_path, is_windows_drive_path,
};

#[derive(Debug, Clone)]
pub(crate) struct RawSpan {
    pub(crate) kind: MarkdownSpanKind,
    pub(crate) byte_start: usize,
    pub(crate) byte_end: usize,
    pub(crate) in_table: bool,
}

pub(crate) fn is_escaped_at(bytes: &[u8], floor: usize, idx: usize) -> bool {
    let mut backslashes = 0_usize;
    let mut pos = idx;
    while pos > floor && bytes[pos - 1] == b'\\' {
        backslashes += 1;
        pos -= 1;
    }
    backslashes % 2 == 1
}

pub(crate) fn skip_code_span(bytes: &[u8], start: usize, end: usize) -> usize {
    let mut ticks = 0_usize;
    while start + ticks < end && bytes[start + ticks] == b'`' {
        ticks += 1;
    }
    let mut i = start + ticks;
    while i < end {
        if bytes[i] == b'`' {
            let mut close_ticks = 0_usize;
            while i + close_ticks < end && bytes[i + close_ticks] == b'`' {
                close_ticks += 1;
            }
            if close_ticks == ticks {
                return i + close_ticks;
            }
            i += close_ticks;
        } else {
            i += 1;
        }
    }
    start + ticks
}

pub(crate) fn try_scan_angle_destination(
    source: &str,
    start: usize,
    end: usize,
    in_table: bool,
    out: &mut Vec<RawSpan>,
) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut i = start + 1;
    while i < end {
        let b = bytes[i];
        if b == b'>' && !is_escaped_at(bytes, start + 1, i) {
            let inner = &source[start + 1..i];
            if is_windows_absolute_path(inner) {
                out.push(RawSpan {
                    kind: MarkdownSpanKind::WindowsPath {
                        path: inner.to_owned(),
                        in_destination: true,
                    },
                    byte_start: start + 1,
                    byte_end: i,
                    in_table,
                });
                return Some(i + 1);
            }
            if inner.starts_with("http://")
                || inner.starts_with("https://")
                || inner.starts_with("mailto:")
            {
                return Some(i + 1);
            }
            return None;
        }
        if b == b'\n' || b == b'\r' || b == b'<' || b.is_ascii_whitespace() {
            return None;
        }
        i += 1;
    }
    None
}

pub(crate) fn try_scan_link_destination(
    source: &str,
    dest_start: usize,
    end: usize,
    in_table: bool,
    out: &mut Vec<RawSpan>,
) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut i = dest_start;
    while i < end && matches!(bytes[i], b' ' | b'\t') {
        i += 1;
    }
    if i >= end {
        return None;
    }
    if bytes[i] == b'<' {
        let angle_start = i + 1;
        i += 1;
        while i < end && bytes[i] != b'>' && bytes[i] != b'\n' && bytes[i] != b'\r' {
            if bytes[i] == b'\\' {
                i += 1;
                if i < end {
                    i += source[i..].chars().next().map_or(1, char::len_utf8);
                }
            } else {
                i += source[i..].chars().next().map_or(1, char::len_utf8);
            }
        }
        if i >= end || bytes[i] != b'>' {
            return None;
        }
        let dest = &source[angle_start..i];
        let close_paren = find_closing_link_paren(bytes, i + 1, end)?;
        if is_windows_absolute_path(dest) {
            out.push(RawSpan {
                kind: MarkdownSpanKind::WindowsPath {
                    path: dest.to_owned(),
                    in_destination: true,
                },
                byte_start: angle_start,
                byte_end: i,
                in_table,
            });
        }
        return Some(close_paren + 1);
    }
    scan_paren_link_destination(source, i, end, in_table, out)
}

fn scan_paren_link_destination(
    source: &str,
    raw_start: usize,
    end: usize,
    in_table: bool,
    out: &mut Vec<RawSpan>,
) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut i = raw_start;
    let mut depth = 0_usize;
    let mut dest_end = None;
    while i < end {
        let b = bytes[i];
        if b == b'\\' && i + 1 < end {
            i += 1;
            i += source[i..].chars().next().map_or(1, char::len_utf8);
            continue;
        }
        if b == b'\n' || b == b'\r' {
            return None;
        }
        if b == b'(' {
            depth += 1;
        } else if b == b')' {
            if depth == 0 {
                let final_end = dest_end.unwrap_or(i);
                let dest = &source[raw_start..final_end];
                if is_windows_absolute_path(dest) {
                    out.push(RawSpan {
                        kind: MarkdownSpanKind::WindowsPath {
                            path: dest.to_owned(),
                            in_destination: true,
                        },
                        byte_start: raw_start,
                        byte_end: final_end,
                        in_table,
                    });
                }
                return Some(i + 1);
            }
            depth -= 1;
        } else if (b == b' ' || b == b'\t') && dest_end.is_none() {
            dest_end = Some(i);
        }
        i += 1;
    }
    None
}

fn find_closing_link_paren(bytes: &[u8], start: usize, end: usize) -> Option<usize> {
    let mut i = start;
    while i < end {
        if bytes[i] == b')' && !is_escaped_at(bytes, start, i) {
            return Some(i);
        }
        if bytes[i] == b'\n' || bytes[i] == b'\r' {
            return None;
        }
        i += 1;
    }
    None
}

pub(crate) fn try_skip_autolink_url(source: &str, start: usize, end: usize) -> Option<usize> {
    if !source.is_char_boundary(start) {
        return None;
    }
    let rest = &source[start..end];
    if !(rest.starts_with("http://") || rest.starts_with("https://") || rest.starts_with("www.")) {
        return None;
    }
    if start > 0 && source.as_bytes()[start - 1].is_ascii_alphanumeric() {
        return None;
    }
    let mut i = start;
    let bytes = source.as_bytes();
    while i < end && !bytes[i].is_ascii_whitespace() && !matches!(bytes[i], b'<' | b'>' | b'`') {
        i += 1;
    }
    Some(i)
}

pub(crate) fn try_scan_prose_windows_path(
    source: &str,
    start: usize,
    end: usize,
    in_table: bool,
    out: &mut Vec<RawSpan>,
) -> Option<usize> {
    if !source.is_char_boundary(start) {
        return None;
    }
    if start > 0 {
        let prev = source.as_bytes()[start - 1];
        if prev.is_ascii_alphanumeric() || prev == b'_' || prev == b'/' || prev == b'\\' {
            return None;
        }
    }
    let rest = &source[start..end];
    let is_drive = is_windows_drive_path(rest) && rest.len() >= 3;
    let is_unc = is_unc_path(rest) && is_prose_unc_share(rest);
    if !is_drive && !is_unc {
        return None;
    }
    let bytes = source.as_bytes();
    let mut i = start;
    let mut parens = 0_usize;
    while i < end {
        let b = bytes[i];
        if b.is_ascii_whitespace() || matches!(b, b'`' | b'<' | b'>' | b'"' | b'\'') {
            break;
        }
        if b == b'(' {
            parens += 1;
        } else if b == b')' {
            if parens == 0 {
                break;
            }
            parens -= 1;
        }
        i += 1;
    }
    while i > start + 3 && matches!(bytes[i - 1], b'.' | b',' | b';' | b':') {
        i -= 1;
    }
    if i <= start + 2 {
        return None;
    }
    out.push(RawSpan {
        kind: MarkdownSpanKind::WindowsPath {
            path: source[start..i].to_owned(),
            in_destination: false,
        },
        byte_start: start,
        byte_end: i,
        in_table,
    });
    Some(i)
}

fn is_prose_unc_share(rest: &str) -> bool {
    let after = rest.strip_prefix("\\\\").unwrap_or("");
    let bytes = after.as_bytes();
    let mut idx = 0_usize;
    while idx < bytes.len()
        && !bytes[idx].is_ascii_whitespace()
        && !matches!(bytes[idx], b'\\' | b'/' | b'(' | b'[' | b'`')
    {
        idx += 1;
    }
    idx > 0 && bytes.get(idx) == Some(&b'\\')
}
