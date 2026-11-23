//! Rich Text Format (`.rtf`) bounded parser matching `documentTextWorker.ts:419-532`.

use std::collections::HashMap;

use crate::doc_extractor::DocInternalErr;
use crate::doc_extractor_rtf_cp::{decode_codepage_byte, font_charset_codepage};
use crate::doc_extractor_xml::TextPart;

#[derive(Clone, Copy)]
struct RtfState {
    skip: bool,
    deleted: bool,
    uc: usize,
    font: i64,
    font_table: bool,
    codepage: u16,
}

impl Default for RtfState {
    fn default() -> Self {
        Self {
            skip: false,
            deleted: false,
            uc: 1,
            font: 0,
            font_table: false,
            codepage: 1252,
        }
    }
}

pub(crate) fn extract_rtf_parts(
    bytes: &[u8],
    parts: &mut Vec<TextPart>,
) -> Result<(), DocInternalErr> {
    if !bytes.starts_with(br"{\rtf") {
        return Err(DocInternalErr::InvalidFormat(
            "Invalid RTF document".to_owned(),
        ));
    }
    let mut state = RtfState::default();
    let mut stack: Vec<RtfState> = Vec::new();
    // A font's \fcharset, not the document code page, encodes the bytes written in that font.
    let mut fonts: HashMap<i64, u16> = HashMap::new();
    let mut default_font = 0i64;
    let mut fallback = 0usize;
    let mut idx = 0usize;

    while idx < bytes.len() {
        let byte = bytes[idx];
        let hidden = state.skip || state.deleted;
        if byte == b'{' || byte == b'}' {
            fallback = 0;
        }
        match byte {
            b'{' => {
                stack.push(state);
                if stack.len() > 128 {
                    return Err(DocInternalErr::Truncated);
                }
                idx += 1;
            }
            b'}' => {
                if let Some(prev) = stack.pop() {
                    state = prev;
                }
                idx += 1;
            }
            b'\\' => {
                handle_rtf_backslash(
                    bytes,
                    &mut idx,
                    &mut state,
                    &mut fonts,
                    &mut default_font,
                    &mut fallback,
                    parts,
                )?;
            }
            b'\n' | b'\r' => idx += 1,
            raw_b => {
                idx += 1;
                if fallback > 0 {
                    fallback -= 1;
                } else if !hidden {
                    push_char(decode_active_byte(&state, &fonts, raw_b), parts);
                }
            }
        }
    }
    Ok(())
}

fn handle_rtf_backslash(
    bytes: &[u8],
    idx: &mut usize,
    state: &mut RtfState,
    fonts: &mut HashMap<i64, u16>,
    default_font: &mut i64,
    fallback: &mut usize,
    parts: &mut Vec<TextPart>,
) -> Result<(), DocInternalErr> {
    let hidden = state.skip || state.deleted;
    *idx += 1;
    if *idx >= bytes.len() {
        return Ok(());
    }
    if bytes[*idx] == b'\''
        && *idx + 2 < bytes.len()
        && let (Some(hi), Some(lo)) = (hex_val(bytes[*idx + 1]), hex_val(bytes[*idx + 2]))
    {
        let val = (hi << 4) | lo;
        if *fallback > 0 {
            *fallback -= 1;
        } else if !hidden {
            push_char(decode_active_byte(state, fonts, val), parts);
        }
        *idx += 3;
        return Ok(());
    }
    if bytes[*idx].is_ascii_alphabetic() {
        let word_start = *idx;
        while *idx < bytes.len() && bytes[*idx].is_ascii_alphabetic() {
            *idx += 1;
        }
        let control = std::str::from_utf8(&bytes[word_start..*idx]).unwrap_or("");
        let param = parse_rtf_param(bytes, idx);
        if *idx < bytes.len() && bytes[*idx] == b' ' {
            *idx += 1;
        }
        if control == "bin" {
            let skip_bytes = usize::try_from(param.unwrap_or(-1)).map_err(|_| {
                DocInternalErr::InvalidFormat("Invalid RTF binary length".to_owned())
            })?;
            *idx = idx.saturating_add(skip_bytes);
        }
        if *fallback > 0 {
            *fallback -= 1;
            return Ok(());
        }
        if is_rtf_skip_destination(control) {
            state.skip = true;
        }
        return apply_rtf_control(control, param, state, fonts, default_font, fallback, parts);
    }

    let sym = bytes[*idx];
    *idx += 1;
    if *fallback > 0 {
        *fallback -= 1;
    } else if sym == b'*' {
        state.skip = true;
    } else if !hidden {
        match sym {
            b'\n' | b'\r' => parts.push(TextPart::Text("\n".to_owned())),
            b'\\' | b'{' | b'}' => parts.push(TextPart::Text(char::from(sym).to_string())),
            b'~' => parts.push(TextPart::Text("\u{00A0}".to_owned())),
            b'_' => parts.push(TextPart::Text("\u{2011}".to_owned())),
            _ => {}
        }
    }
    Ok(())
}

fn apply_rtf_control(
    control: &str,
    param: Option<i64>,
    state: &mut RtfState,
    fonts: &mut HashMap<i64, u16>,
    default_font: &mut i64,
    fallback: &mut usize,
    parts: &mut Vec<TextPart>,
) -> Result<(), DocInternalErr> {
    let hidden = state.skip || state.deleted;
    let value = param.unwrap_or(0);
    match control {
        "fonttbl" => state.font_table = true,
        "deff" => {
            *default_font = value;
            state.font = value;
        }
        "f" => {
            state.font = value;
            if state.font_table {
                fonts.remove(&value);
            }
        }
        "fcharset" => {
            if state.font_table
                && let Some(cp) = font_charset_codepage(value)
            {
                fonts.insert(state.font, cp);
            }
        }
        "deleted" => state.deleted = param != Some(0),
        "plain" => {
            state.deleted = false;
            state.font = *default_font;
        }
        "mac" => state.codepage = 10000,
        "ansicpg" => {
            let cp = u16::try_from(param.unwrap_or(0)).unwrap_or(0);
            if cp != 874 && cp != 10000 && !(1250..=1258).contains(&cp) {
                return Err(DocInternalErr::InvalidFormat(
                    "Unsupported RTF code page".to_owned(),
                ));
            }
            state.codepage = cp;
        }
        "uc" => {
            state.uc = usize::try_from(param.unwrap_or(-1)).map_err(|_| {
                DocInternalErr::InvalidFormat("Invalid RTF fallback count".to_owned())
            })?;
        }
        "u" => {
            if !hidden && let Some(v) = param {
                let code = u32::try_from(if v < 0 { v + 65536 } else { v }).unwrap_or(0xFFFD);
                push_char(
                    char::from_u32(code).unwrap_or(char::REPLACEMENT_CHARACTER),
                    parts,
                );
            }
            *fallback = state.uc;
        }
        "par" | "line" | "sect" | "page" | "column" if !hidden => {
            parts.push(TextPart::Text("\n".to_owned()));
        }
        "tab" if !hidden => parts.push(TextPart::Text("\t".to_owned())),
        "row" if !state.skip => parts.push(TextPart::Text("\n".to_owned())),
        "cell" if !state.skip => parts.push(TextPart::Text("\t".to_owned())),
        _ if !hidden => {
            if let Some(sym) = rtf_symbol(control) {
                parts.push(TextPart::Text(sym.to_owned()));
            }
        }
        _ => {}
    }
    Ok(())
}

fn push_char(ch: char, parts: &mut Vec<TextPart>) {
    let mut buf = [0u8; 4];
    parts.push(TextPart::Text(ch.encode_utf8(&mut buf).to_owned()));
}

fn parse_rtf_param(bytes: &[u8], idx: &mut usize) -> Option<i64> {
    let start = *idx;
    if *idx < bytes.len() && bytes[*idx] == b'-' {
        *idx += 1;
    }
    let digits_start = *idx;
    while *idx < bytes.len() && bytes[*idx].is_ascii_digit() {
        *idx += 1;
    }
    if *idx == digits_start {
        *idx = start;
        return None;
    }
    std::str::from_utf8(&bytes[start..*idx])
        .ok()
        .and_then(|s| s.parse::<i64>().ok())
}

fn is_rtf_skip_destination(control: &str) -> bool {
    matches!(
        control,
        "fonttbl"
            | "colortbl"
            | "stylesheet"
            | "info"
            | "pict"
            | "object"
            | "fldinst"
            | "header"
            | "headerf"
            | "headerl"
            | "headerr"
            | "footer"
            | "footerf"
            | "footerl"
            | "footerr"
    )
}

fn rtf_symbol(control: &str) -> Option<&'static str> {
    let s = match control {
        "endash" => "\u{2013}",
        "emdash" => "\u{2014}",
        "enspace" => "\u{2002}",
        "emspace" => "\u{2003}",
        "qmspace" => "\u{2005}",
        "lquote" => "\u{2018}",
        "rquote" => "\u{2019}",
        "ldblquote" => "\u{201C}",
        "rdblquote" => "\u{201D}",
        "bullet" => "\u{2022}",
        "zwj" => "\u{200D}",
        "zwnj" => "\u{200C}",
        "ltrmark" => "\u{200E}",
        "rtlmark" => "\u{200F}",
        _ => return None,
    };
    Some(s)
}

fn decode_active_byte(state: &RtfState, fonts: &HashMap<i64, u16>, byte: u8) -> char {
    let codepage = fonts.get(&state.font).copied().unwrap_or(state.codepage);
    decode_codepage_byte(codepage, byte)
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
