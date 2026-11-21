//! `SpreadsheetML` (`.xlsx`) worksheet, shared-string, and `ST_Xstring` extraction matching
//! `documentTextWorker.ts:178-192,574-663`.

use std::collections::{BTreeMap, HashMap};

use crate::doc_extractor::DocInternalErr;
use crate::doc_extractor_entities::posix_join;
use crate::doc_extractor_layout::has_cell_text;
use crate::doc_extractor_odf_ws::{all_matching, emit_text, first_matching};
use crate::doc_extractor_office::{read_part, read_relations};
use crate::doc_extractor_xml::{MarkupMode, TextPart, XmlNode, is_hidden_tag};

pub(crate) fn extract_xlsx(
    files: &BTreeMap<String, String>,
    parts: &mut Vec<TextPart>,
    notes: &mut Vec<String>,
) -> Result<(), DocInternalErr> {
    let workbook = read_part(files, "xl/workbook.xml", MarkupMode::Xml, notes)?;
    let date_system = if matches!(
        first_matching(&workbook, "workbookPr")
            .and_then(|n| n.attrs.get("date1904"))
            .map(String::as_str),
        Some("1" | "true")
    ) {
        "1904"
    } else {
        "1900"
    };

    let (custom_formats, xfs_formats) = load_styles(files, notes)?;
    let shared_strings = load_shared_strings(files, notes)?;
    let populated_strings: Vec<bool> = shared_strings.iter().map(has_cell_text).collect();

    let rels = read_relations(files, "xl/_rels/workbook.xml.rels", notes)?;
    let mut sheets = Vec::new();
    all_matching(&workbook, &["sheet"], &mut sheets);

    for sheet in sheets {
        let sheet_name = sheet.attrs.get("name").map_or("", String::as_str);
        parts.push(TextPart::Text(format!("\n## Sheet: {sheet_name}\n")));
        let rel_id = sheet
            .attrs
            .get("r:id")
            .or_else(|| sheet.attrs.get("id"))
            .map(String::as_str);
        let Some(target) = rel_id.and_then(|id| rels.get(id)) else {
            notes.push(format!(
                "\n[Skipped document part: sheet relationship {sheet_name} (missing or invalid XML).]\n"
            ));
            continue;
        };
        let entry_name = posix_join("xl", target);
        let ws = read_part(files, &entry_name, MarkupMode::Xml, notes)?;
        extract_worksheet(
            &ws,
            &shared_strings,
            &populated_strings,
            &xfs_formats,
            &custom_formats,
            date_system,
            parts,
        )?;
    }
    Ok(())
}

fn load_styles(
    files: &BTreeMap<String, String>,
    notes: &mut Vec<String>,
) -> Result<(HashMap<String, String>, Vec<String>), DocInternalErr> {
    let mut custom_formats = HashMap::new();
    let mut xfs_formats = Vec::new();
    if !files.contains_key("xl/styles.xml") {
        return Ok((custom_formats, xfs_formats));
    }
    let styles = read_part(files, "xl/styles.xml", MarkupMode::Xml, notes)?;
    let mut num_fmts = Vec::new();
    all_matching(&styles, &["numFmt"], &mut num_fmts);
    for node in num_fmts {
        if let (Some(id), Some(code)) = (node.attrs.get("numFmtId"), node.attrs.get("formatCode")) {
            custom_formats.insert(id.clone(), code.clone());
        }
    }
    if let Some(xfs) = first_matching(&styles, "cellXfs") {
        for xf in xfs.child_elements() {
            xfs_formats.push(xf.attrs.get("numFmtId").cloned().unwrap_or_default());
        }
    }
    Ok((custom_formats, xfs_formats))
}

fn load_shared_strings(
    files: &BTreeMap<String, String>,
    notes: &mut Vec<String>,
) -> Result<Vec<XmlNode>, DocInternalErr> {
    if !files.contains_key("xl/sharedStrings.xml") {
        return Ok(Vec::new());
    }
    let sst = read_part(files, "xl/sharedStrings.xml", MarkupMode::Xml, notes)?;
    let mut si_refs = Vec::new();
    all_matching(&sst, &["si"], &mut si_refs);
    Ok(si_refs.into_iter().cloned().collect())
}

fn extract_worksheet(
    ws: &XmlNode,
    shared_strings: &[XmlNode],
    populated_strings: &[bool],
    xfs_formats: &[String],
    custom_formats: &HashMap<String, String>,
    date_system: &str,
    parts: &mut Vec<TextPart>,
) -> Result<(), DocInternalErr> {
    let mut rows = Vec::new();
    all_matching(ws, &["row"], &mut rows);
    let mut row_count = 0usize;

    for row in rows {
        let mut row_cells: BTreeMap<usize, &XmlNode> = BTreeMap::new();
        let mut next_col = 0usize;
        let mut columns_truncated = false;
        for cell in row.child_elements().filter(|n| n.name == "c") {
            let col = cell
                .attrs
                .get("r")
                .and_then(|r| parse_xlsx_col(r))
                .unwrap_or(next_col);
            next_col = col + 1;
            if !xlsx_cell_populated(cell, populated_strings) {
                continue;
            }
            if col >= 512 {
                columns_truncated = true;
                continue;
            }
            row_cells.insert(col, cell);
        }
        if row_cells.is_empty() && !columns_truncated {
            continue;
        }
        row_count += 1;
        if row_count > 10_000 {
            return Err(DocInternalErr::Truncated);
        }
        let max_col = row_cells.keys().next_back().copied().unwrap_or(0);
        for col in 0..=max_col {
            if col > 0 {
                parts.push(TextPart::Text("\t".to_owned()));
            }
            if let Some(&cell) = row_cells.get(&col) {
                emit_xlsx_cell(
                    cell,
                    shared_strings,
                    xfs_formats,
                    custom_formats,
                    date_system,
                    parts,
                )?;
            }
        }
        parts.push(TextPart::Text("\n".to_owned()));
        if columns_truncated {
            return Err(DocInternalErr::Truncated);
        }
    }
    Ok(())
}

fn parse_xlsx_col(cell_ref: &str) -> Option<usize> {
    let mut col = 0usize;
    let mut saw_alpha = false;
    for byte in cell_ref.bytes() {
        if byte.is_ascii_uppercase() {
            saw_alpha = true;
            col = col
                .checked_mul(26)?
                .checked_add(usize::from(byte - b'A') + 1)?;
        } else {
            break;
        }
    }
    saw_alpha.then(|| col - 1)
}

fn xlsx_cell_populated(cell: &XmlNode, populated_strings: &[bool]) -> bool {
    let value = first_matching(cell, "v");
    let cell_type = cell.attrs.get("t").map(String::as_str);
    if cell_type == Some("s") {
        let idx_str = value.map(XmlNode::text_only_children).unwrap_or_default();
        return idx_str
            .trim()
            .parse::<usize>()
            .ok()
            .and_then(|idx| populated_strings.get(idx).copied())
            .unwrap_or(false);
    }
    if cell_type == Some("inlineStr") {
        return has_cell_text(cell);
    }
    if value.is_some_and(has_cell_text) {
        return true;
    }
    let mut formulas = Vec::new();
    all_matching(cell, &["f"], &mut formulas);
    formulas.into_iter().any(has_cell_text)
}

fn emit_xlsx_cell(
    cell: &XmlNode,
    shared_strings: &[XmlNode],
    xfs_formats: &[String],
    custom_formats: &HashMap<String, String>,
    date_system: &str,
    parts: &mut Vec<TextPart>,
) -> Result<(), DocInternalErr> {
    let value = first_matching(cell, "v");
    let raw = value.map(XmlNode::text_only_children).unwrap_or_default();
    let cell_type = cell.attrs.get("t").map(String::as_str);

    match cell_type {
        Some("s") => {
            if let Ok(idx) = raw.trim().parse::<usize>()
                && let Some(shared) = shared_strings.get(idx)
            {
                parts.push(TextPart::CellStart);
                emit_xstring_text(shared, parts);
                parts.push(TextPart::CellEnd);
            }
        }
        Some("inlineStr") => {
            parts.push(TextPart::CellStart);
            emit_xstring_text(cell, parts);
            parts.push(TextPart::CellEnd);
        }
        Some("b") if matches!(raw.trim(), "0" | "1") => {
            parts.push(TextPart::CellStart);
            parts.push(TextPart::Text(
                if raw.trim() == "1" { "true" } else { "false" }.to_owned(),
            ));
            parts.push(TextPart::CellEnd);
        }
        _ => {
            if let Some(v) = value
                && has_cell_text(v)
            {
                parts.push(TextPart::CellStart);
                emit_xstring_text(v, parts);
                parts.push(TextPart::CellEnd);
            } else {
                let mut formulas = Vec::new();
                all_matching(cell, &["f"], &mut formulas);
                for f in formulas {
                    parts.push(TextPart::CellStart);
                    emit_text(f, parts)?;
                    parts.push(TextPart::CellEnd);
                }
            }
            let xf_idx = cell
                .attrs
                .get("s")
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(0);
            let format_id = xfs_formats.get(xf_idx).map_or("", String::as_str);
            if matches!(cell_type, None | Some("n"))
                && !raw.trim().is_empty()
                && !format_id.is_empty()
                && format_id != "0"
            {
                let fmt_suffix = custom_formats
                    .get(format_id)
                    .map_or_else(String::new, |fmt| format!("; format={fmt}"));
                parts.push(TextPart::Text(format!(
                    " [Excel numFmtId={format_id}{fmt_suffix}; date system {date_system}]"
                )));
            }
        }
    }
    Ok(())
}

fn emit_xstring_text(node: &XmlNode, out: &mut Vec<TextPart>) {
    if is_hidden_tag(&node.name) {
        return;
    }
    if node.name != "t" && node.name != "v" {
        for child in node.child_elements() {
            emit_xstring_text(child, out);
        }
        return;
    }
    let raw = node.text_only_children();
    out.push(TextPart::Text(decode_xstring(&raw)));
}

pub(crate) fn decode_xstring(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut idx = 0usize;
    while idx < bytes.len() {
        if bytes.len() >= idx + 13
            && bytes[idx..idx + 7].eq_ignore_ascii_case(b"_x005f_")
            && bytes[idx + 7] == b'x'
            && bytes[idx + 8..idx + 12].iter().all(u8::is_ascii_hexdigit)
            && bytes[idx + 12] == b'_'
        {
            out.push('_');
            out.push_str(&input[idx + 7..idx + 13]);
            idx += 13;
            continue;
        }
        if bytes.len() >= idx + 7
            && bytes[idx] == b'_'
            && bytes[idx + 1] == b'x'
            && bytes[idx + 2..idx + 6].iter().all(u8::is_ascii_hexdigit)
            && bytes[idx + 6] == b'_'
            && let Ok(code) = u32::from_str_radix(&input[idx + 2..idx + 6], 16)
            && let Some(ch) = char::from_u32(code)
        {
            out.push(ch);
            idx += 7;
            continue;
        }
        if let Some(ch) = input[idx..].chars().next() {
            out.push(ch);
            idx += ch.len_utf8();
        } else {
            break;
        }
    }
    out
}
