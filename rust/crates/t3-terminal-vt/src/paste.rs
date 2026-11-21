//! Bracketed-paste safety validation and sanitization (`ghostty/vt/paste.h`).
//!
//! Neutralizes embedded bracketed-paste escape markers (`ESC [ 201 ~`, `ESC [ 200 ~`), bare C0
//! control characters other than safe `\n` and `\t`, `DEL` (`0x7F`), C1 controls
//! (`U+0080..=U+009F`), and unsafe `OSC` escape injections (`ESC ] ... BEL/ST`).

const BRACKETED_START: &str = "\x1b[200~";
const BRACKETED_END: &str = "\x1b[201~";

/// Sanitizes untrusted clipboard text before forwarding into a bracketed-paste stream.
///
/// Behavior:
/// - Preserves safe `\n` (`0x0A`) and `\t` (`0x09`) whitespace.
/// - Normalizes `\r\n` by stripping bare `\r` (`0x0D`) while keeping `\n`.
/// - Strips embedded `ESC [ 201 ~` / `ESC [ 200 ~` (including optional leading zeros and C1 `CSI`).
/// - Strips `OSC` escape payloads (`ESC ] ... BEL`, `ESC ] ... ESC \`, C1 `OSC ... ST`, or
///   unterminated `OSC` up to `\n`/EOF).
/// - Strips all remaining bare C0 controls (`0x00..=0x1F` except `\n`/`\t`), `DEL` (`0x7F`), and C1
///   controls (`U+0080..=U+009F`), iterating to a fixed point so split/nested markers cannot
///   re-form after removal.
pub fn sanitize_bracketed_paste(input: &str) -> String {
    let mut current = strip_non_escape_controls(input);
    loop {
        let next = strip_paste_escapes(&current);
        if next.len() == current.len() {
            current = next;
            break;
        }
        current = next;
    }
    strip_remaining_controls(&current)
}

/// Wraps [`sanitize_bracketed_paste`] output in `ESC [ 200 ~ ... ESC [ 201 ~`.
pub fn wrap_bracketed_paste(input: &str) -> String {
    let sanitized = sanitize_bracketed_paste(input);
    let mut out =
        String::with_capacity(BRACKETED_START.len() + sanitized.len() + BRACKETED_END.len());
    out.push_str(BRACKETED_START);
    out.push_str(&sanitized);
    out.push_str(BRACKETED_END);
    out
}

/// Encodes paste input for a terminal PTY (`ghostty_paste_encode` semantics).
///
/// When `bracketed` is `true`, returns [`wrap_bracketed_paste`] output. When `bracketed` is
/// `false`, sanitizes the payload and converts `\n` to `\r` so raw-mode TUIs treat line breaks
/// as Enter rather than `Ctrl+J`.
pub fn encode_paste(input: &str, bracketed: bool) -> String {
    if bracketed {
        return wrap_bracketed_paste(input);
    }
    sanitize_bracketed_paste(input).replace('\n', "\r")
}

/// Returns `true` when `input` contains no newlines, bracketed-paste end markers, `OSC`
/// sequences, or unsafe C0/C1/DEL control characters (`ghostty_paste_is_safe` semantics).
pub fn is_paste_safe(input: &str) -> bool {
    if input.contains(BRACKETED_END) || input.contains("\u{009B}201~") {
        return false;
    }
    !input.chars().any(|ch| match ch {
        '\t' => false,
        '\n' | '\r' | '\x7f' | '\x00'..='\x1f' | '\u{0080}'..='\u{009f}' => true,
        _ => false,
    })
}

fn strip_non_escape_controls(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        if is_structural_escape_or_safe(ch) || !is_unsafe_control(ch) {
            out.push(ch);
        }
    }
    out
}

fn strip_remaining_controls(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        if !is_unsafe_control(ch) {
            out.push(ch);
        }
    }
    out
}

fn is_structural_escape_or_safe(ch: char) -> bool {
    matches!(
        ch,
        '\n' | '\t' | '\x1b' | '\x07' | '\u{009B}' | '\u{009C}' | '\u{009D}'
    )
}

fn is_unsafe_control(ch: char) -> bool {
    match ch {
        '\n' | '\t' => false,
        '\x00'..='\x1f' | '\x7f' | '\u{0080}'..='\u{009f}' => true,
        _ => false,
    }
}

fn strip_paste_escapes(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut idx = 0_usize;
    while idx < bytes.len() {
        if let Some(rest) = input.get(idx..) {
            if let Some(consumed) = match_bracketed_marker_prefix(rest) {
                idx += consumed;
                continue;
            }
            if let Some(consumed) = match_osc_sequence(rest) {
                idx += consumed;
                continue;
            }
            if let Some(ch) = rest.chars().next() {
                out.push(ch);
                idx += ch.len_utf8();
                continue;
            }
        }
        break;
    }
    out
}

fn match_bracketed_marker_prefix(rest: &str) -> Option<usize> {
    let (prefix_len, after_csi) = if let Some(after) = rest.strip_prefix("\x1b[") {
        (2, after)
    } else {
        let after = rest.strip_prefix('\u{009B}')?;
        ('\u{009B}'.len_utf8(), after)
    };
    let zeros = after_csi.bytes().take_while(|b| *b == b'0').count();
    let tail = after_csi.get(zeros..)?;
    if tail.starts_with("201~") || tail.starts_with("200~") {
        Some(prefix_len + zeros + 4)
    } else {
        None
    }
}

fn match_osc_sequence(rest: &str) -> Option<usize> {
    let prefix_len = if rest.starts_with("\x1b]") {
        2
    } else if rest.starts_with('\u{009D}') {
        '\u{009D}'.len_utf8()
    } else {
        return None;
    };
    let tail = rest.get(prefix_len..)?;
    let mut offset = 0_usize;
    let mut iter = tail.char_indices();
    while let Some((pos, ch)) = iter.next() {
        match ch {
            '\x07' => {
                offset = pos + 1;
                return Some(prefix_len + offset);
            }
            '\u{009C}' => {
                offset = pos + ch.len_utf8();
                return Some(prefix_len + offset);
            }
            '\x1b' => {
                if let Some((_, '\\')) = iter.next() {
                    return Some(prefix_len + pos + 2);
                }
                return Some(prefix_len + pos);
            }
            '\n' => {
                // Do not swallow subsequent lines on an unterminated OSC injection.
                return Some(prefix_len + pos);
            }
            _ => {
                offset = pos + ch.len_utf8();
            }
        }
    }
    Some(prefix_len + offset)
}
