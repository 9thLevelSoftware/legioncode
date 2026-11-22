//! Pure Safe Rust tokenizer and language resolver for Verilog, `SystemVerilog`, and VHDL
//! code blocks and diff lines (`f8f4a0b057`).

use legion_surfaces::mobile_diff::{FontStyleFlags, SyntaxTokenInput};
use serde::{Deserialize, Serialize};

use crate::hdl_keywords::is_hdl_keyword;

/// Supported hardware description languages (`f8f4a0b057`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HdlLanguage {
    /// IEEE 1364 Verilog (`.v`, `.vh`, `verilog`).
    Verilog,
    /// IEEE 1800 `SystemVerilog` (`.sv`, `.svh`, `systemverilog`).
    SystemVerilog,
    /// IEEE 1076 VHDL (`.vhd`, `.vhdl`, `vhdl`).
    Vhdl,
}

/// Resolves a language tag, markdown fence token, or file path/extension to an [`HdlLanguage`].
#[must_use]
pub fn resolve_hdl_language(lang_or_path: &str) -> Option<HdlLanguage> {
    let trimmed = lang_or_path.trim().trim_start_matches('.');
    let tail = trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed);
    let candidate = tail.rsplit('.').next().unwrap_or(tail).to_ascii_lowercase();
    match candidate.as_str() {
        "verilog" | "v" | "vh" => Some(HdlLanguage::Verilog),
        "systemverilog" | "system-verilog" | "sv" | "svh" => Some(HdlLanguage::SystemVerilog),
        "vhdl" | "vhd" => Some(HdlLanguage::Vhdl),
        _ => None,
    }
}

/// Tokenizes one source snippet or diff line in Verilog, `SystemVerilog`, or VHDL into
/// `SyntaxTokenInput` runs (comments, strings, based literals, keywords, types, identifiers).
#[must_use]
pub fn tokenize_hdl(lang: HdlLanguage, source: &str) -> Vec<SyntaxTokenInput> {
    let mut out = Vec::new();
    let mut idx = 0_usize;
    while idx < source.len() {
        idx = scan_next_hdl_token(&mut out, lang, source, idx);
    }
    out
}

fn scan_next_hdl_token(
    out: &mut Vec<SyntaxTokenInput>,
    lang: HdlLanguage,
    source: &str,
    idx: usize,
) -> usize {
    let bytes = source.as_bytes();
    let rest = &source[idx..];
    let comment_end = if lang == HdlLanguage::Vhdl && rest.starts_with("--") {
        Some(rest.find('\n').map_or(bytes.len(), |n| idx + n))
    } else if lang != HdlLanguage::Vhdl && rest.starts_with("//") {
        Some(rest.find('\n').map_or(bytes.len(), |n| idx + n))
    } else if lang != HdlLanguage::Vhdl && rest.starts_with("/*") {
        Some(
            rest[2..]
                .find("*/")
                .map_or(bytes.len(), |n| idx + 2 + n + 2),
        )
    } else {
        None
    };
    if let Some(end) = comment_end {
        push_tok(
            out,
            &source[idx..end],
            Some("#6a9955"),
            FontStyleFlags::ITALIC,
        );
        return end;
    }
    if bytes[idx] == b'"' {
        let end = scan_quoted_string(bytes, idx + 1);
        push_tok(
            out,
            &source[idx..end],
            Some("#ce9178"),
            FontStyleFlags::NONE,
        );
        return end;
    }
    if lang == HdlLanguage::Vhdl
        && bytes[idx] == b'\''
        && let Some(end) = scan_vhdl_char_literal(bytes, idx)
    {
        push_tok(
            out,
            &source[idx..end],
            Some("#b5cea8"),
            FontStyleFlags::NONE,
        );
        return end;
    }
    if let Some(end) = scan_hdl_number(bytes, idx, lang) {
        push_tok(
            out,
            &source[idx..end],
            Some("#b5cea8"),
            FontStyleFlags::NONE,
        );
        return end;
    }
    if let Some(first) = rest.chars().next()
        && (first.is_ascii_alphabetic() || first == '_' || first == '$')
    {
        let mut end = idx + first.len_utf8();
        while end < bytes.len()
            && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_' || bytes[end] == b'$')
        {
            end += 1;
        }
        let word = &source[idx..end];
        if is_hdl_keyword(lang, word) {
            push_tok(out, word, Some("#569cd6"), FontStyleFlags::BOLD);
        } else if word.starts_with('$') || word.starts_with('`') {
            push_tok(out, word, Some("#c586c0"), FontStyleFlags::NONE);
        } else {
            push_tok(out, word, Some("#d4d4d4"), FontStyleFlags::NONE);
        }
        return end;
    }
    if bytes[idx] == b'`' && lang != HdlLanguage::Vhdl {
        let mut end = idx + 1;
        while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
            end += 1;
        }
        push_tok(
            out,
            &source[idx..end],
            Some("#c586c0"),
            FontStyleFlags::NONE,
        );
        return end;
    }
    let ch_len = rest.chars().next().map_or(1, char::len_utf8);
    let next = idx + ch_len;
    let color = (!bytes[idx].is_ascii_whitespace()).then_some("#d4d4d4");
    push_tok(out, &source[idx..next], color, FontStyleFlags::NONE);
    next
}

fn push_tok(
    out: &mut Vec<SyntaxTokenInput>,
    content: &str,
    color: Option<&str>,
    font_style: FontStyleFlags,
) {
    let color_owned = color.map(str::to_owned);
    if let Some(last) = out.last_mut()
        && last.color == color_owned
        && last.font_style == font_style
    {
        last.content.push_str(content);
        return;
    }
    out.push(SyntaxTokenInput {
        content: content.to_owned(),
        color: color_owned,
        font_style,
        ..SyntaxTokenInput::default()
    });
}

fn scan_quoted_string(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            i += 2;
            continue;
        }
        if bytes[i] == b'"' || bytes[i] == b'\n' {
            return if bytes[i] == b'"' { i + 1 } else { i };
        }
        i += 1;
    }
    bytes.len()
}

fn scan_vhdl_char_literal(bytes: &[u8], idx: usize) -> Option<usize> {
    (idx + 2 < bytes.len() && bytes[idx + 2] == b'\'').then_some(idx + 3)
}

fn scan_hdl_number(bytes: &[u8], idx: usize, lang: HdlLanguage) -> Option<usize> {
    if lang != HdlLanguage::Vhdl
        && idx + 1 < bytes.len()
        && bytes[idx] == b'\''
        && matches!(bytes[idx + 1], b'0' | b'1' | b'x' | b'X' | b'z' | b'Z')
    {
        return Some(idx + 2);
    }
    if !bytes[idx].is_ascii_digit() {
        return None;
    }
    let mut end = idx;
    while end < bytes.len()
        && (bytes[end].is_ascii_alphanumeric() || matches!(bytes[end], b'\'' | b'_' | b'.'))
    {
        end += 1;
    }
    Some(end)
}
