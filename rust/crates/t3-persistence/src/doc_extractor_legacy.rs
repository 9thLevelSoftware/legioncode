//! HTML/XHTML and Jupyter `.ipynb` document extraction helpers.

use serde_json::Value;

use crate::doc_extractor::DocInternalErr;
use crate::doc_extractor_entities::decode_markup_bytes;
use crate::doc_extractor_layout::emit_paragraphs;
use crate::doc_extractor_xml::{MarkupMode, TextPart, parse_xml};

pub(crate) fn extract_html_parts(
    bytes: &[u8],
    is_xhtml: bool,
    parts: &mut Vec<TextPart>,
) -> Result<(), DocInternalErr> {
    let decoded = decode_markup_bytes(bytes);
    let root = if is_xhtml {
        parse_xml(&decoded, MarkupMode::Xhtml)?
    } else {
        let sanitized = strip_html_raw_blocks(&decoded);
        parse_xml(&sanitized, MarkupMode::Html)?
    };
    emit_paragraphs(&root, parts)
}

fn strip_html_raw_blocks(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut cursor = 0usize;

    while let Some(rel_lt) = input[cursor..].find('<') {
        let tag_start = cursor + rel_lt;
        out.push_str(&input[cursor..tag_start]);
        let after_lt = &bytes[tag_start + 1..];
        let raw_tag: Option<&[u8]> = if starts_ascii_tag(after_lt, b"script") {
            Some(b"</script")
        } else if starts_ascii_tag(after_lt, b"style") {
            Some(b"</style")
        } else {
            None
        };

        if let Some(close_pat) = raw_tag {
            let Some(rel_gt) = input[tag_start..].find('>') else {
                cursor = input.len();
                break;
            };
            let body_start = tag_start + rel_gt + 1;
            if let Some(close_pos) = find_ascii_ci(&bytes[body_start..], close_pat) {
                let close_tag = body_start + close_pos;
                cursor = input[close_tag..]
                    .find('>')
                    .map_or(input.len(), |end_gt| close_tag + end_gt + 1);
            } else {
                cursor = input.len();
            }
        } else {
            out.push('<');
            cursor = tag_start + 1;
        }
    }
    out.push_str(&input[cursor..]);
    out
}

fn starts_ascii_tag(haystack: &[u8], tag: &[u8]) -> bool {
    haystack.len() >= tag.len()
        && haystack[..tag.len()].eq_ignore_ascii_case(tag)
        && haystack
            .get(tag.len())
            .is_none_or(|b| b.is_ascii_whitespace() || *b == b'>' || *b == b'/')
}

fn find_ascii_ci(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if haystack.len() < needle.len() {
        return None;
    }
    (0..=haystack.len() - needle.len()).find(|&idx| {
        haystack[idx..idx + needle.len()].eq_ignore_ascii_case(needle)
            && haystack
                .get(idx + needle.len())
                .is_none_or(|b| b.is_ascii_whitespace() || *b == b'>')
    })
}

pub(crate) fn extract_ipynb_parts(
    raw: &str,
    parts: &mut Vec<TextPart>,
) -> Result<(), DocInternalErr> {
    let book: Value = serde_json::from_str(raw)
        .map_err(|err| DocInternalErr::InvalidFormat(format!("Invalid notebook: {err}")))?;
    let Some(cells) = book.get("cells").and_then(Value::as_array) else {
        return Err(DocInternalErr::InvalidFormat("Invalid notebook".to_owned()));
    };

    for (idx, cell) in cells.iter().enumerate() {
        if idx >= 10_000 {
            return Err(DocInternalErr::Truncated);
        }
        let Some(obj) = cell.as_object() else {
            continue;
        };
        let kind = obj
            .get("cell_type")
            .and_then(Value::as_str)
            .filter(|k| matches!(*k, "code" | "markdown" | "raw"))
            .unwrap_or("markdown");
        parts.push(TextPart::Text(format!("## Cell {} ({kind})\n", idx + 1)));
        if let Some(src) = obj.get("source") {
            push_string_or_lines(src, parts);
        }
        if let Some(outputs) = obj.get("outputs").and_then(Value::as_array) {
            parts.push(TextPart::Text("\nOutput:\n".to_owned()));
            for output in outputs {
                emit_notebook_output(output, parts);
            }
        }
        parts.push(TextPart::Text("\n\n".to_owned()));
    }
    Ok(())
}

fn emit_notebook_output(output: &Value, parts: &mut Vec<TextPart>) {
    let Some(obj) = output.as_object() else {
        return;
    };
    if let Some(text) = obj.get("text") {
        let joined = collect_lines(text);
        parts.push(TextPart::Text(strip_vt_control_sequences(&joined)));
    } else if let Some(plain) = obj
        .get("data")
        .and_then(Value::as_object)
        .and_then(|data| data.get("text/plain"))
    {
        let joined = collect_lines(plain);
        parts.push(TextPart::Text(strip_vt_control_sequences(&joined)));
    } else if obj.get("output_type").and_then(Value::as_str) == Some("error") {
        if let Some(ename) = obj.get("ename") {
            push_string_or_lines(ename, parts);
        }
        if let Some(evalue) = obj.get("evalue") {
            parts.push(TextPart::Text(": ".to_owned()));
            push_string_or_lines(evalue, parts);
        }
        parts.push(TextPart::Text("\n".to_owned()));
        if let Some(traceback) = obj.get("traceback") {
            emit_traceback_lines(traceback, parts);
        }
    }
    parts.push(TextPart::Text("\n".to_owned()));
}

fn emit_traceback_lines(val: &Value, parts: &mut Vec<TextPart>) {
    match val {
        Value::String(s) => {
            parts.push(TextPart::Text(strip_vt_control_sequences(s)));
            parts.push(TextPart::Text("\n".to_owned()));
        }
        Value::Array(arr) => {
            for item in arr {
                if let Some(s) = item.as_str() {
                    parts.push(TextPart::Text(strip_vt_control_sequences(s)));
                    parts.push(TextPart::Text("\n".to_owned()));
                }
            }
        }
        _ => {}
    }
}

fn push_string_or_lines(val: &Value, parts: &mut Vec<TextPart>) {
    match val {
        Value::String(s) => parts.push(TextPart::Text(s.clone())),
        Value::Array(arr) => {
            for item in arr {
                if let Some(s) = item.as_str() {
                    parts.push(TextPart::Text(s.to_owned()));
                }
            }
        }
        _ => {}
    }
}

fn collect_lines(val: &Value) -> String {
    let mut out = String::new();
    match val {
        Value::String(s) => out.push_str(s),
        Value::Array(arr) => {
            for item in arr {
                if let Some(s) = item.as_str() {
                    out.push_str(s);
                }
            }
        }
        _ => {}
    }
    out
}

fn strip_vt_control_sequences(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut iter = input.chars().peekable();
    while let Some(ch) = iter.next() {
        if ch != '\u{001B}' {
            out.push(ch);
            continue;
        }
        match iter.peek().copied() {
            Some('[') => {
                iter.next();
                for next in iter.by_ref() {
                    if ('\u{0040}'..='\u{007E}').contains(&next) {
                        break;
                    }
                }
            }
            Some(']') => {
                iter.next();
                while let Some(next) = iter.next() {
                    if next == '\u{0007}' {
                        break;
                    }
                    if next == '\u{001B}' && iter.peek().copied() == Some('\\') {
                        iter.next();
                        break;
                    }
                }
            }
            Some(_) => {
                iter.next();
            }
            None => {}
        }
    }
    out
}
