//! Paragraph, heading, and bounded table layout emission matching `documentTextWorker.ts:193-283`.

use crate::doc_extractor::DocInternalErr;
use crate::doc_extractor_odf_ws::{all_matching, emit_text, first_matching, repeat_count};
use crate::doc_extractor_xml::{TextPart, XmlChild, XmlNode, is_hidden_tag};

pub(crate) fn emit_paragraphs(
    node: &XmlNode,
    out: &mut Vec<TextPart>,
) -> Result<(), DocInternalErr> {
    if is_hidden_tag(&node.name) {
        return Ok(());
    }
    for child in &node.children {
        match child {
            XmlChild::Text(t) => out.push(TextPart::Text(t.clone())),
            XmlChild::Element(el) => {
                if matches!(el.name.as_str(), "br" | "line-break" | "cr") {
                    out.push(TextPart::Text("\n".to_owned()));
                } else if matches!(el.name.as_str(), "tbl" | "table") {
                    emit_table(el, out)?;
                } else if is_heading_or_paragraph(&el.name) {
                    let level = heading_level(el);
                    if level > 0 {
                        out.push(TextPart::Text(format!("{} ", "#".repeat(level.min(6)))));
                    }
                    emit_text(el, out)?;
                    out.push(TextPart::Text("\n".to_owned()));
                } else {
                    emit_paragraphs(el, out)?;
                    if matches!(el.name.as_str(), "li" | "div" | "section") {
                        out.push(TextPart::Text("\n".to_owned()));
                    }
                }
            }
        }
    }
    Ok(())
}

fn is_heading_or_paragraph(name: &str) -> bool {
    name == "p" || name == "h" || parse_h_level(name).is_some()
}

fn parse_h_level(name: &str) -> Option<usize> {
    let bytes = name.as_bytes();
    if bytes.len() == 2 && bytes[0] == b'h' && (b'1'..=b'6').contains(&bytes[1]) {
        return Some(usize::from(bytes[1] - b'0'));
    }
    None
}

fn heading_level(node: &XmlNode) -> usize {
    if node.name == "h" {
        return node
            .attrs
            .get("outline-level")
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|&n| n > 0)
            .unwrap_or(1);
    }
    if let Some(level) = parse_h_level(&node.name) {
        return level;
    }
    let style = first_matching(node, "pStyle")
        .and_then(|el| el.attrs.get("val"))
        .map_or("", String::as_str);
    if style.len() == "Heading1".len()
        && style[..7].eq_ignore_ascii_case("heading")
        && let Some(b @ b'1'..=b'6') = style.as_bytes().get(7)
    {
        return usize::from(b - b'0');
    }
    0
}

pub(crate) fn emit_table(node: &XmlNode, out: &mut Vec<TextPart>) -> Result<(), DocInternalErr> {
    let mut rows = Vec::new();
    all_matching(node, &["tr", "table-row"], &mut rows);
    let mut row_count = 0usize;

    for row in rows {
        // Word nests a cell inside w:sdt when a content control wraps the whole cell (#40496).
        let mut cells = Vec::new();
        all_matching(
            row,
            &["tc", "table-cell", "covered-table-cell", "td", "th"],
            &mut cells,
        );
        while cells.last().is_some_and(|c| !has_cell_text(c)) {
            cells.pop();
        }
        if cells.is_empty() {
            continue;
        }
        let repeat = repeat_count(row.attrs.get("number-rows-repeated").map(String::as_str))?;
        for _ in 0..repeat {
            row_count += 1;
            if row_count > 10_000 {
                return Err(DocInternalErr::Truncated);
            }
            let mut column = 0usize;
            for &cell in &cells {
                let count = repeat_count(
                    cell.attrs
                        .get("number-columns-repeated")
                        .map(String::as_str),
                )?;
                for _ in 0..count {
                    if column >= 512 {
                        return Err(DocInternalErr::Truncated);
                    }
                    if column > 0 {
                        out.push(TextPart::Text("\t".to_owned()));
                    }
                    column += 1;
                    out.push(TextPart::CellStart);
                    emit_paragraphs(cell, out)?;
                    out.push(TextPart::Fallback(odf_value(cell)));
                    out.push(TextPart::CellEnd);
                }
            }
            out.push(TextPart::Text("\n".to_owned()));
        }
    }
    Ok(())
}

pub(crate) fn odf_value(cell: &XmlNode) -> String {
    let kind = cell
        .attrs
        .get("office:value-type")
        .or_else(|| cell.attrs.get("value-type"))
        .map(String::as_str);
    let attr = |name: &str| {
        cell.attrs
            .get(&format!("office:{name}"))
            .or_else(|| cell.attrs.get(name))
            .map_or("", String::as_str)
    };
    match kind {
        Some("void") => String::new(),
        Some("string") => attr("string-value").to_owned(),
        Some("boolean") => attr("boolean-value").to_owned(),
        Some("date") => attr("date-value").to_owned(),
        Some("time") => attr("time-value").to_owned(),
        Some("currency") => {
            let v = attr("value");
            if v.trim().is_empty() {
                String::new()
            } else {
                format!("{v} {}", attr("currency")).trim().to_owned()
            }
        }
        Some("percentage") => {
            let v = attr("value");
            if v.trim().is_empty() {
                String::new()
            } else {
                format!("{v} [ODF value-type=percentage]")
            }
        }
        _ => attr("value").to_owned(),
    }
}

pub(crate) fn has_cell_text(node: &XmlNode) -> bool {
    if !odf_value(node).trim().is_empty() || node.name == "noBreakHyphen" {
        return true;
    }
    if is_hidden_tag(&node.name) || node.space_count.is_some() {
        return false;
    }
    node.children.iter().any(|child| match child {
        XmlChild::Text(t) => !t.trim().is_empty(),
        XmlChild::Element(el) => has_cell_text(el),
    })
}
