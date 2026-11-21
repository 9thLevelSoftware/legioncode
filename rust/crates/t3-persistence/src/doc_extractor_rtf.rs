//! Rich Text Format (`.rtf`) bounded parser matching `documentTextWorker.ts:419-532`.

use crate::doc_extractor::DocInternalErr;
use crate::doc_extractor_xml::TextPart;

#[derive(Clone, Copy)]
struct RtfState {
    skip: bool,
    deleted: bool,
    uc: usize,
    codepage: u16,
}

impl Default for RtfState {
    fn default() -> Self {
        Self {
            skip: false,
            deleted: false,
            uc: 1,
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
                handle_rtf_backslash(bytes, &mut idx, &mut state, &mut fallback, parts)?;
            }
            b'\n' | b'\r' => idx += 1,
            raw_b => {
                idx += 1;
                if fallback > 0 {
                    fallback -= 1;
                } else if !hidden {
                    push_char(decode_rtf_byte(state.codepage, raw_b)?, parts);
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
            push_char(decode_rtf_byte(state.codepage, val)?, parts);
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
        return apply_rtf_control(control, param, state, fallback, parts);
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
    fallback: &mut usize,
    parts: &mut Vec<TextPart>,
) -> Result<(), DocInternalErr> {
    let hidden = state.skip || state.deleted;
    match control {
        "deleted" => state.deleted = param != Some(0),
        "plain" => state.deleted = false,
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

fn decode_rtf_byte(codepage: u16, byte: u8) -> Result<char, DocInternalErr> {
    if byte < 0x80 {
        return Ok(char::from(byte));
    }
    if codepage == 1252 {
        const CP1252_80_9F: [char; 32] = [
            '\u{20AC}', '\u{FFFD}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}',
            '\u{2021}', '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{FFFD}',
            '\u{017D}', '\u{FFFD}', '\u{FFFD}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}',
            '\u{2022}', '\u{2013}', '\u{2014}', '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}',
            '\u{0153}', '\u{FFFD}', '\u{017E}', '\u{0178}',
        ];
        return Ok(if byte < 0xA0 {
            CP1252_80_9F[usize::from(byte - 0x80)]
        } else {
            char::from(byte)
        });
    }
    if codepage == 1251 {
        const CP1251_80_BF: [char; 64] = [
            '\u{0402}', '\u{0403}', '\u{201A}', '\u{0453}', '\u{201E}', '\u{2026}', '\u{2020}',
            '\u{2021}', '\u{20AC}', '\u{2030}', '\u{0409}', '\u{2039}', '\u{040A}', '\u{040C}',
            '\u{040B}', '\u{040F}', '\u{0452}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}',
            '\u{2022}', '\u{2013}', '\u{2014}', '\u{FFFD}', '\u{2122}', '\u{0459}', '\u{203A}',
            '\u{045A}', '\u{045C}', '\u{045B}', '\u{045F}', '\u{00A0}', '\u{040E}', '\u{045E}',
            '\u{0408}', '\u{00A4}', '\u{0490}', '\u{00A6}', '\u{00A7}', '\u{0401}', '\u{00A9}',
            '\u{0404}', '\u{00AB}', '\u{00AC}', '\u{00AD}', '\u{00AE}', '\u{0407}', '\u{00B0}',
            '\u{00B1}', '\u{0406}', '\u{0456}', '\u{0491}', '\u{00B5}', '\u{00B6}', '\u{00B7}',
            '\u{0451}', '\u{2116}', '\u{0454}', '\u{00BB}', '\u{0458}', '\u{0405}', '\u{0455}',
            '\u{0457}',
        ];
        return if byte < 0xC0 {
            let mapped = CP1251_80_BF[usize::from(byte - 0x80)];
            if mapped == '\u{FFFD}' {
                Err(DocInternalErr::InvalidFormat(
                    "Invalid byte for cp1251".to_owned(),
                ))
            } else {
                Ok(mapped)
            }
        } else {
            Ok(char::from_u32(0x0410 + u32::from(byte - 0xC0))
                .unwrap_or(char::REPLACEMENT_CHARACTER))
        };
    }
    Ok(char::from(byte))
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
