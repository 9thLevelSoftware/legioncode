#![forbid(unsafe_code)]
//! Strict Safe Rust YAML & Markdown-frontmatter subset parser for `.legion-cli/` artifacts.

use serde_json::{Map, Value};

use crate::workflow_board_cli_yaml_flow::{
    find_mapping_colon, parse_scalar_or_flow, strip_unquoted_comment, unquote_key,
};

#[derive(Debug, Clone)]
struct Line<'a> {
    no: usize,
    indent: usize,
    has_tab_indent: bool,
    text: &'a str,
    raw: &'a str,
    is_ignorable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockStyle {
    Literal,
    Folded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockChomp {
    Clip,
    Strip,
    Keep,
}

#[derive(Debug, Clone, Copy)]
struct BlockScalarHeader {
    style: BlockStyle,
    chomp: BlockChomp,
    explicit_indent: Option<usize>,
}

/// Parses a `.legion-cli` YAML file into a [`serde_json::Value`] tree, rejecting malformed syntax.
pub(crate) fn parse_yaml_str(raw: &str, path_label: &str) -> Result<Value, String> {
    let lines = preprocess_lines(raw);
    let mut idx = 0_usize;
    let Some(first) = peek_sig(&lines, &mut idx, path_label)? else {
        return Ok(Value::Object(Map::new()));
    };
    let base_indent = first.indent;
    let val = parse_node(&lines, &mut idx, base_indent, path_label)?;
    if let Some(trailing) = peek_sig(&lines, &mut idx, path_label)? {
        return Err(format!(
            "Invalid YAML in `{path_label}` at line {}: unexpected trailing content `{}`",
            trailing.no, trailing.text
        ));
    }
    Ok(val)
}

/// Splits `---\n...\n---` Markdown frontmatter from its body and parses the YAML header.
pub(crate) fn parse_markdown_frontmatter(
    raw: &str,
    path_label: &str,
) -> Result<(Option<Value>, String), String> {
    let normalized = raw.replace("\r\n", "\n");
    let trimmed_start = normalized.trim_start_matches('\u{feff}');
    if let Some(after_open) = trimmed_start.strip_prefix("---\n") {
        let mut offset = 0_usize;
        let mut close_span = None;
        for line in after_open.split_inclusive('\n') {
            let clean = line.trim_end_matches('\n').trim();
            if clean == "---" {
                close_span = Some((offset, offset + line.len()));
                break;
            }
            offset += line.len();
        }
        if let Some((yaml_end, body_start)) = close_span {
            let yaml_part = &after_open[..yaml_end];
            let body_part = after_open[body_start..].trim().to_owned();
            let parsed = parse_yaml_str(yaml_part, path_label)?;
            return Ok((Some(parsed), body_part));
        }
        return Err(format!(
            "Malformed YAML frontmatter in `{path_label}`: missing closing `---`"
        ));
    }
    Ok((None, normalized.trim().to_owned()))
}

fn preprocess_lines(raw: &str) -> Vec<Line<'_>> {
    raw.lines()
        .enumerate()
        .map(|(idx, raw_line)| {
            let no = idx + 1;
            let without_cr = raw_line.trim_end_matches('\r');
            let space_indent = without_cr.bytes().take_while(|b| *b == b' ').count();
            let any_ws = without_cr
                .bytes()
                .take_while(|b| *b == b' ' || *b == b'\t')
                .count();
            let has_tab_indent = without_cr[..any_ws].contains('\t');
            let stripped_comment = strip_unquoted_comment(without_cr);
            let trimmed = stripped_comment.trim();
            let is_ignorable = trimmed.is_empty() || trimmed == "---" || trimmed == "...";
            Line {
                no,
                indent: space_indent,
                has_tab_indent,
                text: if is_ignorable {
                    ""
                } else {
                    stripped_comment[any_ws..].trim_end()
                },
                raw: without_cr,
                is_ignorable,
            }
        })
        .collect()
}

fn peek_sig<'a>(
    lines: &'a [Line<'a>],
    idx: &mut usize,
    path_label: &str,
) -> Result<Option<&'a Line<'a>>, String> {
    while *idx < lines.len() && lines[*idx].is_ignorable {
        *idx += 1;
    }
    let Some(line) = lines.get(*idx) else {
        return Ok(None);
    };
    if line.has_tab_indent {
        return Err(format!(
            "Invalid YAML in `{path_label}` at line {}: tab indentation is not supported",
            line.no
        ));
    }
    Ok(Some(line))
}

fn is_seq_item(text: &str) -> bool {
    text == "-" || text.starts_with("- ")
}

fn parse_node(
    lines: &[Line<'_>],
    idx: &mut usize,
    indent: usize,
    path_label: &str,
) -> Result<Value, String> {
    let Some(line) = peek_sig(lines, idx, path_label)? else {
        return Ok(Value::Null);
    };
    if is_seq_item(line.text) {
        parse_sequence(lines, idx, indent, path_label)
    } else if find_mapping_colon(line.text).is_some() {
        parse_mapping(lines, idx, indent, path_label)
    } else {
        *idx += 1;
        parse_scalar_or_flow(line.text, line.no, path_label)
    }
}

fn parse_sequence(
    lines: &[Line<'_>],
    idx: &mut usize,
    seq_indent: usize,
    path_label: &str,
) -> Result<Value, String> {
    let mut items = Vec::new();
    while let Some(line) = peek_sig(lines, idx, path_label)? {
        if line.indent < seq_indent || (line.indent == seq_indent && !is_seq_item(line.text)) {
            break;
        }
        if line.indent != seq_indent {
            return Err(format!(
                "Invalid YAML indentation in `{path_label}` at line {}",
                line.no
            ));
        }
        let ws_after_dash = line.text[1..].bytes().take_while(|b| *b == b' ').count();
        let rest = &line.text[1 + ws_after_dash..];
        let line_no = line.no;
        *idx += 1;
        if rest.is_empty() {
            items.push(parse_indented_or_null(lines, idx, seq_indent, path_label)?);
        } else if let Some(hdr) = parse_block_scalar_header(rest) {
            items.push(parse_block_scalar_body(
                lines, idx, seq_indent, hdr, path_label,
            )?);
        } else if let Some(colon) = find_mapping_colon(rest) {
            let item_indent = seq_indent + 1 + ws_after_dash;
            let map = parse_sequence_item_mapping(
                lines,
                idx,
                seq_indent,
                item_indent,
                rest,
                colon,
                line_no,
                path_label,
            )?;
            items.push(Value::Object(map));
        } else {
            items.push(parse_scalar_or_flow(rest, line_no, path_label)?);
        }
    }
    Ok(Value::Array(items))
}

#[expect(
    clippy::too_many_arguments,
    reason = "Parses inline and continuation mapping entries under a YAML sequence dash"
)]
fn parse_sequence_item_mapping(
    lines: &[Line<'_>],
    idx: &mut usize,
    seq_indent: usize,
    item_indent: usize,
    first_rest: &str,
    first_colon: usize,
    first_line_no: usize,
    path_label: &str,
) -> Result<Map<String, Value>, String> {
    let mut map = Map::new();
    let key = unquote_key(&first_rest[..first_colon], first_line_no, path_label)?;
    let after = first_rest[first_colon + 1..].trim_start();
    let val = parse_value_after_colon(
        lines,
        idx,
        item_indent,
        item_indent,
        after,
        first_line_no,
        path_label,
    )?;
    map.insert(key, val);

    while let Some(entry_line) = peek_sig(lines, idx, path_label)? {
        if entry_line.indent <= seq_indent {
            break;
        }
        if entry_line.indent != item_indent {
            return Err(format!(
                "Invalid YAML indentation in `{path_label}` at line {}",
                entry_line.no
            ));
        }
        if is_seq_item(entry_line.text) {
            break;
        }
        let Some(c2) = find_mapping_colon(entry_line.text) else {
            return Err(format!(
                "Invalid YAML mapping in `{path_label}` at line {}",
                entry_line.no
            ));
        };
        let k2 = unquote_key(&entry_line.text[..c2], entry_line.no, path_label)?;
        let a2 = entry_line.text[c2 + 1..].trim_start();
        let lno2 = entry_line.no;
        *idx += 1;
        let v2 =
            parse_value_after_colon(lines, idx, item_indent, item_indent, a2, lno2, path_label)?;
        map.insert(k2, v2);
    }
    Ok(map)
}

fn parse_mapping(
    lines: &[Line<'_>],
    idx: &mut usize,
    map_indent: usize,
    path_label: &str,
) -> Result<Value, String> {
    let mut map = Map::new();
    while let Some(line) = peek_sig(lines, idx, path_label)? {
        if line.indent < map_indent {
            break;
        }
        if line.indent > map_indent {
            return Err(format!(
                "Invalid YAML indentation in `{path_label}` at line {}",
                line.no
            ));
        }
        if is_seq_item(line.text) {
            break;
        }
        let Some(colon) = find_mapping_colon(line.text) else {
            return Err(format!(
                "Malformed YAML mapping entry in `{path_label}` at line {}: `{}`",
                line.no, line.text
            ));
        };
        let key = unquote_key(&line.text[..colon], line.no, path_label)?;
        let after = line.text[colon + 1..].trim_start();
        let line_no = line.no;
        *idx += 1;
        let val = parse_value_after_colon(
            lines, idx, map_indent, map_indent, after, line_no, path_label,
        )?;
        map.insert(key, val);
    }
    Ok(Value::Object(map))
}

fn parse_value_after_colon(
    lines: &[Line<'_>],
    idx: &mut usize,
    key_indent: usize,
    block_parent_indent: usize,
    after: &str,
    line_no: usize,
    path_label: &str,
) -> Result<Value, String> {
    if after.is_empty() {
        parse_nested_or_same_indent_seq(lines, idx, key_indent, path_label)
    } else if let Some(hdr) = parse_block_scalar_header(after) {
        parse_block_scalar_body(lines, idx, block_parent_indent, hdr, path_label)
    } else {
        parse_scalar_or_flow(after, line_no, path_label)
    }
}

fn parse_indented_or_null(
    lines: &[Line<'_>],
    idx: &mut usize,
    parent_indent: usize,
    path_label: &str,
) -> Result<Value, String> {
    if let Some(next) = peek_sig(lines, idx, path_label)?
        && next.indent > parent_indent
    {
        parse_node(lines, idx, next.indent, path_label)
    } else {
        Ok(Value::Null)
    }
}

fn parse_nested_or_same_indent_seq(
    lines: &[Line<'_>],
    idx: &mut usize,
    key_indent: usize,
    path_label: &str,
) -> Result<Value, String> {
    let Some(next) = peek_sig(lines, idx, path_label)? else {
        return Ok(Value::Null);
    };
    if next.indent > key_indent {
        parse_node(lines, idx, next.indent, path_label)
    } else if next.indent == key_indent && is_seq_item(next.text) {
        parse_sequence(lines, idx, key_indent, path_label)
    } else {
        Ok(Value::Null)
    }
}

fn parse_block_scalar_header(raw_after: &str) -> Option<BlockScalarHeader> {
    let cleaned = strip_unquoted_comment(raw_after).trim();
    let mut chars = cleaned.chars();
    let style = match chars.next()? {
        '|' => BlockStyle::Literal,
        '>' => BlockStyle::Folded,
        _ => return None,
    };
    let mut chomp = BlockChomp::Clip;
    let mut explicit_indent = None;
    let mut seen_chomp = false;
    let mut seen_indent = false;
    for ch in chars {
        match ch {
            '-' if !seen_chomp => {
                chomp = BlockChomp::Strip;
                seen_chomp = true;
            }
            '+' if !seen_chomp => {
                chomp = BlockChomp::Keep;
                seen_chomp = true;
            }
            '1'..='9' if !seen_indent => {
                explicit_indent = ch.to_digit(10).map(|d| d as usize);
                seen_indent = true;
            }
            _ => return None,
        }
    }
    Some(BlockScalarHeader {
        style,
        chomp,
        explicit_indent,
    })
}

fn parse_block_scalar_body(
    lines: &[Line<'_>],
    idx: &mut usize,
    parent_indent: usize,
    hdr: BlockScalarHeader,
    path_label: &str,
) -> Result<Value, String> {
    let block_indent = if let Some(n) = hdr.explicit_indent {
        parent_indent + n
    } else {
        let mut probe = *idx;
        while probe < lines.len() && lines[probe].raw.trim().is_empty() {
            probe += 1;
        }
        match lines.get(probe) {
            Some(first) if first.indent > parent_indent && !first.has_tab_indent => first.indent,
            Some(first) if first.has_tab_indent && first.indent <= parent_indent => {
                return Err(format!(
                    "Invalid YAML in `{path_label}` at line {}: tab indentation is not supported",
                    first.no
                ));
            }
            _ => {
                *idx = probe;
                return Ok(Value::String(String::new()));
            }
        }
    };

    let mut collected: Vec<String> = Vec::new();
    let mut trailing_blanks = 0_usize;
    while *idx < lines.len() {
        let line = &lines[*idx];
        if line.raw.trim().is_empty() {
            if line.indent > block_indent && !line.has_tab_indent {
                for _ in 0..trailing_blanks {
                    collected.push(String::new());
                }
                trailing_blanks = 0;
                collected.push(line.raw[block_indent..].to_owned());
            } else {
                trailing_blanks += 1;
            }
            *idx += 1;
            continue;
        }
        if line.indent <= parent_indent {
            break;
        }
        if line.has_tab_indent || line.indent < block_indent {
            return Err(format!(
                "Invalid YAML block scalar indentation in `{path_label}` at line {}",
                line.no
            ));
        }
        for _ in 0..trailing_blanks {
            collected.push(String::new());
        }
        trailing_blanks = 0;
        collected.push(line.raw[block_indent..].to_owned());
        *idx += 1;
    }

    Ok(Value::String(render_block_scalar(
        &collected,
        trailing_blanks,
        hdr,
    )))
}

fn render_block_scalar(lines: &[String], trailing_blanks: usize, hdr: BlockScalarHeader) -> String {
    if lines.is_empty() {
        return match hdr.chomp {
            BlockChomp::Keep => "\n".repeat(trailing_blanks),
            BlockChomp::Clip | BlockChomp::Strip => String::new(),
        };
    }
    let mut out = match hdr.style {
        BlockStyle::Literal => lines.join("\n"),
        BlockStyle::Folded => fold_block_lines(lines),
    };
    match hdr.chomp {
        BlockChomp::Strip => {}
        BlockChomp::Clip => out.push('\n'),
        BlockChomp::Keep => {
            out.push('\n');
            for _ in 0..trailing_blanks {
                out.push('\n');
            }
        }
    }
    out
}

fn fold_block_lines(lines: &[String]) -> String {
    let mut out = String::new();
    let mut prev_was_plain = false;
    let mut prev_was_more_indented = false;
    for line in lines {
        if line.is_empty() {
            out.push('\n');
            prev_was_plain = false;
            prev_was_more_indented = false;
            continue;
        }
        let more_indented = line.starts_with(' ') || line.starts_with('\t');
        if more_indented {
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(line);
            prev_was_plain = false;
            prev_was_more_indented = true;
        } else {
            if prev_was_plain {
                out.push(' ');
            } else if prev_was_more_indented {
                out.push('\n');
            }
            out.push_str(line);
            prev_was_plain = true;
            prev_was_more_indented = false;
        }
    }
    out
}

#[cfg(test)]
#[path = "workflow_board_cli_yaml_tests.rs"]
mod tests;
