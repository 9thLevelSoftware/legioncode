//! Low-level C1, OSC/DCS terminator, CSI boundary, and UTF-8 char-boundary helpers for
//! [`crate::history_sanitizer`].

pub(crate) struct C1Effect {
    pub(crate) next: String,
    pub(crate) query: bool,
    pub(crate) safe: String,
}

pub(crate) struct C1Hit {
    pub(crate) effect: C1Effect,
    pub(crate) index: usize,
}

pub(crate) struct StringEnd {
    pub(crate) end: usize,
    pub(crate) c1: Option<C1Hit>,
}

pub(crate) fn c1_effect(ch: char) -> Option<C1Effect> {
    let mut utf8 = [0_u8; 4];
    let bytes = ch.encode_utf8(&mut utf8).as_bytes();
    let mut next: Option<String> = None;
    let mut query = false;
    let mut safe = String::new();
    for &byte in bytes {
        match next.as_deref() {
            Some("") => safe.push('\u{FFFD}'),
            Some(s) if s.starts_with("\x1b]") => {
                if let Some(buf) = next.as_mut() {
                    buf.push('\u{FFFD}');
                }
            }
            _ => match byte {
                0x9B => next = Some("\x1b[".to_owned()),
                0x9D => next = Some("\x1b]".to_owned()),
                0x90 => next = Some("\x1bP".to_owned()),
                0x98 | 0x9E | 0x9F => next = Some("\x1b_".to_owned()),
                0x80..=0x9F => {
                    next = Some(String::new());
                    query = byte == 0x9A;
                    if !query && byte != 0x9C {
                        safe = format!("\x1b{}", char::from(byte - 0x40));
                    } else {
                        safe.clear();
                    }
                }
                _ => {}
            },
        }
    }
    next.map(|n| C1Effect {
        next: n,
        query,
        safe,
    })
}

pub(crate) fn find_string_end(input: &str, start: usize, is_osc: bool) -> Option<StringEnd> {
    for (rel, ch) in input[start..].char_indices() {
        let abs = start + rel;
        if (is_osc && ch == '\x07') || is_cancel_control(ch) {
            return Some(StringEnd {
                end: abs + 1,
                c1: None,
            });
        }
        if ch == '\x1b' {
            let next = input.as_bytes().get(abs + 1).copied()?;
            let end = if next == b'\\' { abs + 2 } else { abs };
            return Some(StringEnd { end, c1: None });
        }
        if !is_osc
            && ch as u32 >= 0x80
            && let Some(effect) = c1_effect(ch)
        {
            return Some(StringEnd {
                end: abs + ch.len_utf8(),
                c1: Some(C1Hit { effect, index: abs }),
            });
        }
    }
    None
}

pub(crate) fn find_escape_sequence_end(input: &str, start: usize) -> Option<usize> {
    let mut cursor = start;
    let bytes = input.as_bytes();
    while cursor < bytes.len() && (0x20..=0x2F).contains(&bytes[cursor]) {
        cursor += 1;
    }
    if cursor >= bytes.len() {
        return None;
    }
    if bytes[cursor] == b'\x1b' {
        return Some(cursor);
    }
    Some(if (0x30..=0x7E).contains(&bytes[cursor]) {
        cursor + 1
    } else {
        start + 1
    })
}

pub(crate) fn strip_string_terminator(value: &str) -> &str {
    if let Some(stripped) = value.strip_suffix("\x1b\\") {
        return stripped;
    }
    value.strip_suffix(['\x07', '\u{009C}']).unwrap_or(value)
}

pub(crate) fn terminated_osc_or_dcs(slice: &str) -> bool {
    slice.ends_with('\x07')
        || slice.ends_with('\x18')
        || slice.ends_with('\x1a')
        || slice.ends_with("\x1b\\")
}

pub(crate) const fn is_csi_final(ch: char) -> bool {
    (ch as u32) >= 0x40 && (ch as u32) <= 0x7E
}

pub(crate) const fn is_cancel_control(ch: char) -> bool {
    ch == '\x18' || ch == '\x1a'
}

pub(crate) fn floor_char_boundary(s: &str, idx: usize) -> usize {
    let mut pos = idx.min(s.len());
    while pos > 0 && !s.is_char_boundary(pos) {
        pos -= 1;
    }
    pos
}
