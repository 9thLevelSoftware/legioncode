#![forbid(unsafe_code)]
//! Codepoint-bounded PII, secret, and local-path scrubber for local agent feedback (`#35871`).

use std::collections::HashSet;
use std::sync::OnceLock;

use regex::Regex;

use crate::trace_types::{looks_random, next_char_boundary, utf8_char_len};

/// Default maximum Unicode codepoints kept per scrubbed feedback field.
pub const FEEDBACK_TEXT_MAX_CHARS: usize = 300;
/// Replacement marker inserted for redacted values, secrets, and paths.
pub const REDACTED: &str = "[redacted]";

/// Scrubbed feedback string along with truncation status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScrubbedText {
    /// Scrubbed text capped at `max_chars` Unicode scalar values.
    pub text: String,
    /// True when scan window truncation or final length cap dropped characters.
    pub cut: bool,
}

struct Patterns {
    token_prefix: Regex,
    jwt: Regex,
    credential_url: Regex,
    email: Regex,
    long_run: Regex,
}

fn patterns() -> &'static Patterns {
    static PATS: OnceLock<Patterns> = OnceLock::new();
    PATS.get_or_init(|| {
        // Static patterns verified at module startup.
        Patterns {
            token_prefix: Regex::new(
                r"\b(?:sk-|sk_live_|pk_live_|rk_live_|gh[pousr]_|github_pat_|glpat-|xox[abprs]-|xapp-|npm_|hf_|AIza|AKIA|ASIA)[A-Za-z0-9_-]{8,}",
            )
            .unwrap_or_else(|_| unreachable!()),
            jwt: Regex::new(r"\beyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+")
                .unwrap_or_else(|_| unreachable!()),
            credential_url: Regex::new(r"(?i)\b[a-z][a-z0-9+.-]*://[^\s/@]+@\S+")
                .unwrap_or_else(|_| unreachable!()),
            email: Regex::new(
                r"[A-Za-z0-9.!#$%&'*+=?^_`{|}~-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}",
            )
            .unwrap_or_else(|_| unreachable!()),
            long_run: Regex::new(r"(?:[A-Za-z0-9_+=-]|\[redacted\])+")
                .unwrap_or_else(|_| unreachable!()),
        }
    })
}

/// Scrubs credentials, secrets, emails, known sensitive context strings, local filesystem paths,
/// and high-entropy runs, capping output to [`FEEDBACK_TEXT_MAX_CHARS`] Unicode scalar values.
pub fn scrub_feedback(text: &str, sensitive: &[&str]) -> String {
    scrub_feedback_text(text, sensitive, FEEDBACK_TEXT_MAX_CHARS).text
}

/// Same as [`scrub_feedback`], returning explicit `max_chars` capping and truncation metadata.
pub fn scrub_feedback_text(text: &str, sensitive: &[&str], max_chars: usize) -> ScrubbedText {
    let window = max_chars.saturating_mul(4);
    let mut scrubbed = String::new();
    let mut next_ch: Option<char> = None;
    for (idx, ch) in text.chars().enumerate() {
        if idx == window {
            next_ch = Some(ch);
            break;
        }
        scrubbed.push(ch);
    }
    if let Some(nc) = next_ch
        && nc != '\n'
    {
        let keep_to = scrubbed.rfind('\n').map_or(0, |i| i + 1);
        scrubbed.truncate(keep_to);
        scrubbed.push_str(REDACTED);
    }

    let known = collect_known_values(sensitive);
    let step1 = redact_secrets_and_known(&scrubbed, &known);
    let step2 = redact_quoted_paths(&step1);
    let step3 = redact_unquoted_paths(&step2);
    let step4 = redact_long_runs(&step3);

    let total_chars = step4.chars().count();
    let capped: String = step4.chars().take(max_chars).collect();
    ScrubbedText {
        text: capped,
        cut: next_ch.is_some() || total_chars > max_chars,
    }
}

fn collect_known_values(sensitive: &[&str]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for raw in sensitive {
        let trimmed = raw.trim();
        if trimmed.chars().count() < 3 {
            continue;
        }
        for line in trimmed.split('\n') {
            let line = line.trim();
            if !line.is_empty() && seen.insert(line.to_owned()) {
                out.push(line.to_owned());
            }
        }
    }
    out
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn redact_secrets_and_known(text: &str, known: &[String]) -> String {
    let pats = patterns();
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for re in [
        &pats.credential_url,
        &pats.token_prefix,
        &pats.jwt,
        &pats.email,
    ] {
        for m in re.find_iter(text) {
            spans.push((m.start(), m.end()));
        }
    }
    let bytes = text.as_bytes();
    for val in known {
        let vb = val.as_bytes();
        let mut search_from = 0usize;
        while let Some(rel) = text[search_from..].find(val.as_str()) {
            let start = search_from + rel;
            let end = start + vb.len();
            let left_ok = start == 0 || !is_word_byte(bytes[start - 1]);
            let right_ok = end == bytes.len() || !is_word_byte(bytes[end]);
            if left_ok && right_ok {
                spans.push((start, end));
            }
            search_from = next_char_boundary(text, start);
        }
    }
    apply_redacted_spans(text, spans)
}

fn apply_redacted_spans(text: &str, mut spans: Vec<(usize, usize)>) -> String {
    if spans.is_empty() {
        return text.to_owned();
    }
    spans.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
    let mut out = String::with_capacity(text.len());
    let mut at = 0usize;
    for (start, end) in spans {
        if start < at {
            at = at.max(end);
            continue;
        }
        out.push_str(&text[at..start]);
        out.push_str(REDACTED);
        at = end;
    }
    out.push_str(&text[at..]);
    out
}

fn redact_quoted_paths(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'"' || b == b'\'' || b == b'`' {
            let quote = b;
            let rest = &text[i + 1..];
            if starts_quoted_path(rest, quote)
                && let Some(close_rel) =
                    rest.find(|c: char| u32::from(quote) == c as u32 || c == '\n')
                && rest.as_bytes()[close_rel] == quote
            {
                out.push(quote as char);
                out.push_str(REDACTED);
                out.push(quote as char);
                i += 1 + close_rel + 1;
                continue;
            }
        }
        let ch_len = utf8_char_len(text, i);
        out.push_str(&text[i..i + ch_len]);
        i += ch_len;
    }
    out
}

fn starts_quoted_path(rest: &str, quote: u8) -> bool {
    if rest.starts_with("~/")
        || rest.starts_with("file://")
        || rest.starts_with(r"\\")
        || rest.starts_with("[redacted]/")
        || rest.starts_with(r"[redacted]\")
    {
        return true;
    }
    let b = rest.as_bytes();
    if b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'/' || b[2] == b'\\')
    {
        return true;
    }
    if let Some(after) = rest.strip_prefix('/') {
        for ch in after.chars() {
            if ch == '/' {
                return true;
            }
            if ch as u32 == u32::from(quote) || ch == '\n' {
                return false;
            }
        }
    }
    false
}

fn redact_unquoted_paths(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0usize;
    while i < bytes.len() {
        if let Some(matched) = match_path_start(text, i) {
            let _ = matched;
            out.push_str(REDACTED);
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        let ch_len = utf8_char_len(text, i);
        out.push_str(&text[i..i + ch_len]);
        i += ch_len;
    }
    out
}

fn match_path_start(text: &str, i: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let tail = &text[i..];
    if tail.starts_with("[redacted]://") {
        return Some(i + "[redacted]://".len());
    }
    if tail.starts_with("[redacted]/") || tail.starts_with(r"[redacted]\") {
        return Some(i + REDACTED.len());
    }
    if tail.starts_with("~/") {
        let prev_bad = i > 0 && (is_word_byte(bytes[i - 1]) || matches!(bytes[i - 1], b'~' | b'-'));
        if !prev_bad {
            return Some(i + 2);
        }
    }
    if tail.starts_with("file://") {
        let prev_word = i > 0 && is_word_byte(bytes[i - 1]);
        if !prev_word {
            return Some(i + 7);
        }
    }
    if tail.len() >= 3
        && bytes[i].is_ascii_alphabetic()
        && bytes[i + 1] == b':'
        && (bytes[i + 2] == b'/' || bytes[i + 2] == b'\\')
    {
        let prev_word = i > 0 && is_word_byte(bytes[i - 1]);
        if !prev_word {
            return Some(i + 3);
        }
    }
    if let Some(after) = tail.strip_prefix(r"\\") {
        let prev_bad = i > 0 && (is_word_byte(bytes[i - 1]) || bytes[i - 1] == b'\\');
        if !prev_bad
            && let Some(slash) = after.find('\\')
            && slash > 0
            && !after[..slash]
                .chars()
                .any(|c| c.is_whitespace() || c == '/')
        {
            return Some(i + 2 + slash + 1);
        }
    }
    if let Some(after) = tail.strip_prefix('/') {
        let prev_bad = i > 0
            && (is_word_byte(bytes[i - 1])
                || matches!(bytes[i - 1], b'.' | b':' | b'/' | b'~' | b'-'));
        if !prev_bad && let Some(slash) = after.find('/') {
            let seg = &after[..slash];
            let bad_seg = seg.is_empty()
                || seg.chars().any(|c| {
                    c.is_whitespace()
                        || matches!(
                            c,
                            '/' | '\\'
                                | '"'
                                | '\''
                                | '`'
                                | '<'
                                | '>'
                                | '('
                                | ')'
                                | '{'
                                | '}'
                                | ','
                                | ';'
                                | ':'
                                | '|'
                        )
                });
            let after_slash = &after[slash + 1..];
            let next_non_ws = after_slash
                .chars()
                .next()
                .is_some_and(|c| !c.is_whitespace());
            if !bad_seg && next_non_ws {
                return Some(i + 1 + slash + 1);
            }
        }
    }
    None
}

fn redact_long_runs(text: &str) -> String {
    let re = &patterns().long_run;
    let mut out = String::with_capacity(text.len());
    let mut at = 0usize;
    for m in re.find_iter(text) {
        out.push_str(&text[at..m.start()]);
        let run = m.as_str();
        if run.chars().count() >= 32 && (run.contains(REDACTED) || looks_random(run)) {
            out.push_str(REDACTED);
        } else {
            out.push_str(run);
        }
        at = m.end();
    }
    out.push_str(&text[at..]);
    out
}
