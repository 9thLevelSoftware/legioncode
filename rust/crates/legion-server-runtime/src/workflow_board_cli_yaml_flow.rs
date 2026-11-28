#![forbid(unsafe_code)]
//! Flow collection and scalar/colon helpers for [`super::workflow_board_cli_yaml`].

use serde_json::{Map, Value};

pub(super) fn strip_unquoted_comment(line: &str) -> &str {
    let mut in_single = false;
    let mut in_double = false;
    let mut escape = false;
    let bytes = line.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if escape {
            escape = false;
            continue;
        }
        if in_double && b == b'\\' {
            escape = true;
            continue;
        }
        if !in_single && b == b'"' {
            in_double = !in_double;
        } else if !in_double && b == b'\'' {
            in_single = !in_single;
        } else if !in_single
            && !in_double
            && b == b'#'
            && (i == 0 || bytes[i - 1].is_ascii_whitespace())
        {
            return &line[..i];
        }
    }
    line
}

pub(super) fn find_mapping_colon(text: &str) -> Option<usize> {
    let mut in_single = false;
    let mut in_double = false;
    let mut escape = false;
    let mut bracket_depth = 0_usize;
    let mut brace_depth = 0_usize;
    let bytes = text.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if escape {
            escape = false;
            continue;
        }
        if in_double && b == b'\\' {
            escape = true;
            continue;
        }
        if !in_single && b == b'"' {
            in_double = !in_double;
        } else if !in_double && b == b'\'' {
            in_single = !in_single;
        } else if !in_single && !in_double {
            match b {
                b'[' => bracket_depth += 1,
                b']' => bracket_depth = bracket_depth.saturating_sub(1),
                b'{' => brace_depth += 1,
                b'}' => brace_depth = brace_depth.saturating_sub(1),
                b':' if bracket_depth == 0 && brace_depth == 0 => {
                    let next_is_boundary =
                        i + 1 == bytes.len() || bytes[i + 1].is_ascii_whitespace();
                    let prev_was_quote = i > 0 && matches!(bytes[i - 1], b'"' | b'\'');
                    if next_is_boundary || prev_was_quote {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }
    }
    None
}

pub(super) fn unquote_key(raw: &str, line_no: usize, path_label: &str) -> Result<String, String> {
    let t = raw.trim();
    if t.is_empty() {
        return Err(format!(
            "Empty YAML key in `{path_label}` at line {line_no}"
        ));
    }
    if t.starts_with('"') || t.starts_with('\'') {
        match parse_scalar_or_flow(t, line_no, path_label)? {
            Value::String(s) => Ok(s),
            _ => Err(format!(
                "Invalid YAML key in `{path_label}` at line {line_no}"
            )),
        }
    } else {
        Ok(t.to_owned())
    }
}

pub(super) fn parse_scalar_or_flow(
    raw: &str,
    line_no: usize,
    path_label: &str,
) -> Result<Value, String> {
    let t = raw.trim();
    if t.is_empty() || t == "null" || t == "~" {
        return Ok(Value::Null);
    }
    if t == "true" {
        return Ok(Value::Bool(true));
    }
    if t == "false" {
        return Ok(Value::Bool(false));
    }
    if t.starts_with('[') {
        let Some(close_idx) = find_matching_delimiter(t, b'[', b']') else {
            return Err(format!(
                "Malformed YAML flow sequence in `{path_label}` at line {line_no}: unterminated `[`"
            ));
        };
        if close_idx != t.len() - 1 {
            return Err(format!(
                "Malformed YAML flow sequence in `{path_label}` at line {line_no}: unexpected trailing tokens"
            ));
        }
        let inner = t[1..close_idx].trim();
        if inner.is_empty() {
            return Ok(Value::Array(Vec::new()));
        }
        let mut items = Vec::new();
        for part in split_flow_items(inner, line_no, path_label)? {
            items.push(parse_scalar_or_flow(part, line_no, path_label)?);
        }
        return Ok(Value::Array(items));
    }
    if t.starts_with('{') {
        let Some(close_idx) = find_matching_delimiter(t, b'{', b'}') else {
            return Err(format!(
                "Malformed YAML flow mapping in `{path_label}` at line {line_no}: unterminated `{{`"
            ));
        };
        if close_idx != t.len() - 1 {
            return Err(format!(
                "Malformed YAML flow mapping in `{path_label}` at line {line_no}: unexpected trailing tokens"
            ));
        }
        let inner = t[1..close_idx].trim();
        if inner.is_empty() {
            return Ok(Value::Object(Map::new()));
        }
        let mut map = Map::new();
        for part in split_flow_items(inner, line_no, path_label)? {
            let Some(colon) = find_mapping_colon(part) else {
                return Err(format!(
                    "Malformed YAML flow mapping entry in `{path_label}` at line {line_no}: `{part}`"
                ));
            };
            let key = unquote_key(&part[..colon], line_no, path_label)?;
            let val_raw = part[colon + 1..].trim();
            let val = if val_raw.is_empty() {
                Value::Null
            } else {
                parse_scalar_or_flow(val_raw, line_no, path_label)?
            };
            map.insert(key, val);
        }
        return Ok(Value::Object(map));
    }
    if t.starts_with('"') {
        return serde_json::from_str::<String>(t)
            .map(Value::String)
            .map_err(|e| {
                format!("Malformed YAML string in `{path_label}` at line {line_no}: {e}")
            });
    }
    if t.starts_with('\'') {
        if t.len() < 2 || !t.ends_with('\'') {
            return Err(format!(
                "Malformed YAML single-quoted string in `{path_label}` at line {line_no}"
            ));
        }
        return Ok(Value::String(t[1..t.len() - 1].replace("''", "'")));
    }
    if let Ok(n) = t.parse::<i64>() {
        return Ok(Value::Number(n.into()));
    }
    if t.contains('.')
        && let Ok(f) = t.parse::<f64>()
        && let Some(num) = serde_json::Number::from_f64(f)
    {
        return Ok(Value::Number(num));
    }
    Ok(Value::String(t.to_owned()))
}

fn find_matching_delimiter(text: &str, open: u8, close: u8) -> Option<usize> {
    let mut depth = 0_usize;
    let mut in_single = false;
    let mut in_double = false;
    let mut escape = false;
    for (i, &b) in text.as_bytes().iter().enumerate() {
        if escape {
            escape = false;
            continue;
        }
        if in_double && b == b'\\' {
            escape = true;
            continue;
        }
        if !in_single && b == b'"' {
            in_double = !in_double;
        } else if !in_double && b == b'\'' {
            in_single = !in_single;
        } else if !in_single && !in_double {
            if b == open {
                depth += 1;
            } else if b == close {
                if depth == 0 {
                    return None;
                }
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
        }
    }
    None
}

fn split_flow_items<'a>(
    inner: &'a str,
    line_no: usize,
    path_label: &str,
) -> Result<Vec<&'a str>, String> {
    let mut out = Vec::new();
    let mut start = 0_usize;
    let mut in_single = false;
    let mut in_double = false;
    let mut escape = false;
    let mut bracket_depth = 0_usize;
    let mut brace_depth = 0_usize;
    for (i, b) in inner.bytes().enumerate() {
        if escape {
            escape = false;
            continue;
        }
        if in_double && b == b'\\' {
            escape = true;
            continue;
        }
        if !in_single && b == b'"' {
            in_double = !in_double;
        } else if !in_double && b == b'\'' {
            in_single = !in_single;
        } else if !in_single && !in_double {
            match b {
                b'[' => bracket_depth += 1,
                b']' => {
                    if bracket_depth == 0 {
                        return Err(format!(
                            "Malformed YAML flow collection in `{path_label}` at line {line_no}: unexpected `]`"
                        ));
                    }
                    bracket_depth -= 1;
                }
                b'{' => brace_depth += 1,
                b'}' => {
                    if brace_depth == 0 {
                        return Err(format!(
                            "Malformed YAML flow collection in `{path_label}` at line {line_no}: unexpected `}}`"
                        ));
                    }
                    brace_depth -= 1;
                }
                b',' if bracket_depth == 0 && brace_depth == 0 => {
                    let piece = inner[start..i].trim();
                    if piece.is_empty() {
                        return Err(format!(
                            "Empty YAML flow item in `{path_label}` at line {line_no}"
                        ));
                    }
                    out.push(piece);
                    start = i + 1;
                }
                _ => {}
            }
        }
    }
    if in_single || in_double || bracket_depth != 0 || brace_depth != 0 {
        return Err(format!(
            "Unterminated YAML flow collection in `{path_label}` at line {line_no}"
        ));
    }
    let tail = inner[start..].trim();
    if !tail.is_empty() {
        out.push(tail);
    }
    Ok(out)
}
