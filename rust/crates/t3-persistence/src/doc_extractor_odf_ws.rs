//! ODF §6.1.2 whitespace collapsing, node tree queries, and inline text emitters.

use crate::doc_extractor::{DOC_MAX_OUTPUT_CHARS, DocInternalErr};
use crate::doc_extractor_xml::{TextPart, XmlChild, XmlNode, is_hidden_tag};

pub(crate) fn collapse_odf_whitespace(paragraph: &mut XmlNode) {
    let mut after_space = true;
    let mut tail_coords: Option<(Vec<usize>, String)> = None;
    let mut path: Vec<usize> = Vec::new();
    visit_odf_ws(paragraph, &mut after_space, &mut tail_coords, &mut path);
    if after_space
        && let Some((coords, value)) = tail_coords
        && let Some(trimmed) = value.strip_suffix(' ')
    {
        set_text_at(paragraph, &coords, trimmed.to_owned());
    }
}

fn visit_odf_ws(
    node: &mut XmlNode,
    after_space: &mut bool,
    tail: &mut Option<(Vec<usize>, String)>,
    path: &mut Vec<usize>,
) {
    for (idx, child) in node.children.iter_mut().enumerate() {
        match child {
            XmlChild::Element(el) => {
                if is_hidden_tag(&el.name) {
                    continue;
                }
                if el.space_count.is_none()
                    && !matches!(el.name.as_str(), "tab" | "line-break" | "p" | "h")
                {
                    path.push(idx);
                    visit_odf_ws(el, after_space, tail, path);
                    path.pop();
                } else {
                    *after_space = false;
                    *tail = None;
                }
            }
            XmlChild::Text(raw) => {
                let mut collapsed = collapse_ascii_runs(raw);
                if *after_space && collapsed.starts_with(' ') {
                    collapsed.remove(0);
                }
                if !collapsed.is_empty() {
                    *after_space = collapsed.ends_with(' ');
                    let mut full_path = path.clone();
                    full_path.push(idx);
                    *tail = Some((full_path, collapsed.clone()));
                }
                *raw = collapsed;
            }
        }
    }
}

fn set_text_at(mut node: &mut XmlNode, path: &[usize], new_val: String) {
    for (depth, &idx) in path.iter().enumerate() {
        let Some(child) = node.children.get_mut(idx) else {
            return;
        };
        if depth + 1 == path.len() {
            if let XmlChild::Text(text) = child {
                *text = new_val;
            }
            return;
        }
        match child {
            XmlChild::Element(el) => node = el,
            XmlChild::Text(_) => return,
        }
    }
}

fn collapse_ascii_runs(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_ws = false;
    for ch in input.chars() {
        if matches!(ch, ' ' | '\t' | '\r' | '\n') {
            if !in_ws {
                out.push(' ');
                in_ws = true;
            }
        } else {
            in_ws = false;
            out.push(ch);
        }
    }
    out
}

pub(crate) fn repeat_count(value: Option<&str>) -> Result<usize, DocInternalErr> {
    let Some(raw) = value else {
        return Ok(1);
    };
    if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return Err(DocInternalErr::InvalidFormat(
            "invalid repeat count".to_owned(),
        ));
    }
    let parsed = raw
        .parse::<usize>()
        .map_err(|_| DocInternalErr::Truncated)?;
    if parsed < 1 {
        return Err(DocInternalErr::InvalidFormat("repeat count < 1".to_owned()));
    }
    Ok(parsed)
}

pub(crate) fn all_matching<'a>(node: &'a XmlNode, names: &[&str], out: &mut Vec<&'a XmlNode>) {
    for child in node.child_elements() {
        if names.contains(&child.name.as_str()) {
            out.push(child);
        } else {
            all_matching(child, names, out);
        }
    }
}

pub(crate) fn first_matching<'a>(node: &'a XmlNode, name: &str) -> Option<&'a XmlNode> {
    for child in node.child_elements() {
        if child.name == name {
            return Some(child);
        }
        if let Some(found) = first_matching(child, name) {
            return Some(found);
        }
    }
    None
}

pub(crate) fn emit_text(node: &XmlNode, out: &mut Vec<TextPart>) -> Result<(), DocInternalErr> {
    if is_hidden_tag(&node.name) {
        return Ok(());
    }
    match node.name.as_str() {
        "tab" | "ptab" | "cell" => out.push(TextPart::Text("\t".to_owned())),
        "br" | "line-break" | "cr" => out.push(TextPart::Text("\n".to_owned())),
        "noBreakHyphen" => out.push(TextPart::Text("\u{2011}".to_owned())),
        _ => {
            if let Some(count) = node.space_count {
                let capped = count.min(DOC_MAX_OUTPUT_CHARS);
                out.push(TextPart::Text(" ".repeat(capped)));
                if count > DOC_MAX_OUTPUT_CHARS {
                    return Err(DocInternalErr::Truncated);
                }
            } else {
                for child in &node.children {
                    match child {
                        XmlChild::Text(t) => out.push(TextPart::Text(t.clone())),
                        XmlChild::Element(el) => emit_text(el, out)?,
                    }
                }
            }
        }
    }
    Ok(())
}
