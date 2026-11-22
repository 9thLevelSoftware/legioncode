//! Parser and structure-preserving upserter for `<base_dir>/service.env` files.
//!
//! Mirrors `upsertServiceEnv` and `NodeUtil.parseEnv` usage in `apps/server/src/cli/service.ts:45-97`.

use std::collections::{BTreeMap, HashSet};

/// Parses `service.env` contents using Node `util.parseEnv`-compatible quoting and comment rules.
///
/// Later definitions of the same key overwrite earlier ones. Comments (`# ...`) and invalid
/// lines are ignored, while single/double/backtick-quoted multiline values are preserved.
pub fn parse_service_env(contents: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let lines: Vec<&str> = contents.split('\n').collect();
    let mut index = 0usize;
    let mut offset = 0usize;

    while index < lines.len() {
        let line = lines[index];
        let Some((key, after_eq)) = parse_env_assignment_prefix(line) else {
            offset += line.len() + 1;
            index += 1;
            continue;
        };

        let rest = line[after_eq..].trim_start_matches([' ', '\t', '\r']);
        let first_char = rest.chars().next();
        if let Some(quote @ ('"' | '\'' | '`')) = first_char {
            let open_in_line = line.len() - rest.len();
            let after_open = offset + open_in_line + 1;
            if let Some((raw_value, end_idx, span_len)) =
                read_quoted_value(contents, after_open, &lines, index, quote)
            {
                let value = if quote == '"' {
                    unescape_double_quoted(&raw_value)
                } else {
                    raw_value
                };
                out.insert(key.to_owned(), value);
                offset += span_len;
                index = end_idx;
                continue;
            }
        }

        let unquoted = strip_inline_comment(rest).trim_end_matches([' ', '\t', '\r']);
        out.insert(key.to_owned(), unquoted.to_owned());
        offset += line.len() + 1;
        index += 1;
    }

    out
}

/// Sets or removes (`None`) entries in `service.env` text while keeping every other line,
/// comment, and multiline quoted block intact (`service.ts:45-86`).
///
/// # Examples
///
/// ```
/// use legion_persistence::service_env::upsert_service_env;
///
/// let before = "# user settings\nLEGIONCODE_PORT=4000\nexport LEGIONCODE_LAN=0\n";
/// let after = upsert_service_env(before, &[("LEGIONCODE_LAN", Some("true")), ("LEGIONCODE_HOST", None)]);
/// assert_eq!(after, "# user settings\nLEGIONCODE_PORT=4000\nLEGIONCODE_LAN=true\n");
/// ```
pub fn upsert_service_env(contents: &str, entries: &[(&str, Option<&str>)]) -> String {
    let entry_map: BTreeMap<&str, Option<&str>> = entries.iter().copied().collect();
    let mut written: HashSet<&str> = HashSet::new();
    let source: Vec<&str> = contents.split('\n').collect();
    let mut lines: Vec<String> = Vec::with_capacity(source.len() + entries.len());

    let mut index = 0usize;
    let mut offset = 0usize;
    while index < source.len() {
        let line = source[index];
        let prefix = parse_env_assignment_prefix(line);
        let mut end = index + 1;
        if let Some((_, after_eq)) = prefix {
            // Match Node `util.parseEnv`: only ASCII space, tab, and CR count before an opening quote.
            let rest = line[after_eq..].trim_start_matches([' ', '\t', '\r']);
            if let Some(quote @ ('"' | '\'' | '`')) = rest.chars().next()
                && !rest[1..].contains(quote)
            {
                let search_from = offset + line.len();
                if let Some(rel_close) = contents[search_from..].find(quote) {
                    let closing = search_from + rel_close;
                    end += contents[search_from..closing].matches('\n').count();
                }
            }
        }

        let span = &source[index..end];
        offset += span.iter().map(|part| part.len() + 1).sum::<usize>();
        index = end;

        let Some((key, _)) = prefix else {
            lines.push(span.join("\n"));
            continue;
        };
        let Some(entry_val) = entry_map.get(key) else {
            lines.push(span.join("\n"));
            continue;
        };
        let Some(value) = entry_val else {
            continue;
        };
        if !written.insert(key) {
            continue;
        }
        lines.push(format!("{key}={value}"));
    }

    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    for &(key, maybe_val) in entries {
        if let Some(value) = maybe_val
            && written.insert(key)
        {
            lines.push(format!("{key}={value}"));
        }
    }

    if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join("\n"))
    }
}

/// Convenience wrapper for [`upsert_service_env`] accepting an owned `BTreeMap`.
pub fn upsert_service_env_map(
    contents: &str,
    entries: &BTreeMap<String, Option<String>>,
) -> String {
    let borrowed: Vec<(&str, Option<&str>)> = entries
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_deref()))
        .collect();
    upsert_service_env(contents, &borrowed)
}

fn parse_env_assignment_prefix(line: &str) -> Option<(&str, usize)> {
    let mut pos = skip_unicode_whitespace(line, 0);
    if line[pos..].starts_with("export") {
        let after_export = pos + "export".len();
        let after_ws = skip_unicode_whitespace(line, after_export);
        if after_ws > after_export {
            pos = after_ws;
        }
    }

    let key_start = pos;
    let bytes = line.as_bytes();
    let first = *bytes.get(pos)?;
    if !(first.is_ascii_alphabetic() || first == b'_') {
        return None;
    }
    pos += 1;
    while pos < bytes.len() && (bytes[pos].is_ascii_alphanumeric() || bytes[pos] == b'_') {
        pos += 1;
    }
    let key = &line[key_start..pos];
    pos = skip_unicode_whitespace(line, pos);
    if bytes.get(pos).copied() != Some(b'=') {
        return None;
    }
    Some((key, pos + 1))
}

fn skip_unicode_whitespace(s: &str, mut pos: usize) -> usize {
    while let Some(ch) = s[pos..].chars().next() {
        if ch.is_whitespace() {
            pos += ch.len_utf8();
        } else {
            break;
        }
    }
    pos
}

fn read_quoted_value(
    contents: &str,
    after_open: usize,
    lines: &[&str],
    start_idx: usize,
    quote: char,
) -> Option<(String, usize, usize)> {
    let mut search_pos = after_open;
    while let Some(rel) = contents[search_pos..].find(quote) {
        let abs = search_pos + rel;
        if quote == '"' && is_escaped_backslash(contents, after_open, abs) {
            search_pos = abs + 1;
            continue;
        }
        let raw = contents[after_open..abs].to_owned();
        let line_span = contents[after_open..abs].matches('\n').count() + 1;
        let end_idx = (start_idx + line_span).min(lines.len());
        let span_len = lines[start_idx..end_idx]
            .iter()
            .map(|part| part.len() + 1)
            .sum::<usize>();
        return Some((raw, end_idx, span_len));
    }
    None
}

fn is_escaped_backslash(s: &str, start: usize, quote_pos: usize) -> bool {
    let mut count = 0usize;
    let bytes = s.as_bytes();
    let mut idx = quote_pos;
    while idx > start {
        idx -= 1;
        if bytes[idx] == b'\\' {
            count += 1;
        } else {
            break;
        }
    }
    count % 2 == 1
}

fn unescape_double_quoted(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\'
            && let Some(next) = chars.next()
        {
            match next {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                '\\' => out.push('\\'),
                '"' => out.push('"'),
                other => {
                    out.push('\\');
                    out.push(other);
                }
            }
        } else {
            out.push(ch);
        }
    }
    out
}

fn strip_inline_comment(rest: &str) -> &str {
    match rest.find('#') {
        Some(idx) => &rest[..idx],
        None => rest,
    }
}
