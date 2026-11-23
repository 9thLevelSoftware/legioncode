//! Recursive JSON Schema validation helpers (`check_output_json` & `validate_json_schema_value`).

use serde_json::{Map, Value};

const MAX_SCHEMA_DEPTH: usize = 32;

/// Validates JSON text against a JSON Schema document (supporting `anyOf`, `$defs`/`$ref`,
/// `type`, `required`, `properties`, `additionalProperties`, `items`, `enum`, `const`, and bounds).
pub fn check_output_json(
    schema: &Map<String, Value>,
    json_text: &str,
    strict_excess: bool,
) -> Result<Value, String> {
    let parsed: Value =
        serde_json::from_str(json_text).map_err(|_| "`json` is not valid JSON text.".to_owned())?;
    validate_json_schema_value(schema, &parsed, strict_excess)?;
    Ok(parsed)
}

/// Validates a parsed JSON `value` against `schema`.
pub fn validate_json_schema_value(
    schema: &Map<String, Value>,
    value: &Value,
    strict_excess: bool,
) -> Result<(), String> {
    let defs = schema.get("$defs").and_then(Value::as_object);
    validate_value_with_defs(schema, defs, value, "$", 0, strict_excess)
}

fn validate_value_with_defs(
    schema: &Map<String, Value>,
    defs: Option<&Map<String, Value>>,
    value: &Value,
    path: &str,
    depth: usize,
    strict_excess: bool,
) -> Result<(), String> {
    if depth > MAX_SCHEMA_DEPTH {
        return Err(format!("{path}: exceeded maximum nesting depth"));
    }
    if let Some(Value::String(reference)) = schema.get("$ref")
        && let Some(def_name) = reference.strip_prefix("#/$defs/")
        && let Some(target) = defs
            .and_then(|d| d.get(def_name))
            .and_then(Value::as_object)
    {
        return validate_value_with_defs(target, defs, value, path, depth + 1, strict_excess);
    }
    if let Some(Value::Array(variants)) = schema.get("anyOf") {
        let mut errors = Vec::new();
        for variant in variants {
            if let Some(sub) = variant.as_object() {
                match validate_value_with_defs(sub, defs, value, path, depth + 1, strict_excess) {
                    Ok(()) => return Ok(()),
                    Err(err) => errors.push(err),
                }
            }
        }
        return Err(errors
            .into_iter()
            .next()
            .unwrap_or_else(|| format!("{path}: value did not match anyOf")));
    }
    if let Some(expected_const) = schema.get("const")
        && value != expected_const
    {
        return Err(format!("{path}: value does not match `const`"));
    }
    if let Some(Value::Array(variants)) = schema.get("enum")
        && !variants.contains(value)
    {
        return Err(format!("{path}: value is not one of allowed enum options"));
    }
    if let Some(ty) = schema.get("type")
        && !matches_schema_type(ty, value)
    {
        return Err(format!("{path}: expected type {ty}"));
    }
    match value {
        Value::Object(obj) => {
            validate_object(schema, defs, obj, path, depth, strict_excess)?;
        }
        Value::Array(arr) => validate_array(schema, defs, arr, path, depth, strict_excess)?,
        Value::String(s) => validate_string(schema, s, path)?,
        Value::Number(num) => validate_number(schema, num, path)?,
        Value::Bool(_) | Value::Null => {}
    }
    Ok(())
}

fn validate_object(
    schema: &Map<String, Value>,
    defs: Option<&Map<String, Value>>,
    obj: &Map<String, Value>,
    path: &str,
    depth: usize,
    strict_excess: bool,
) -> Result<(), String> {
    if let Some(Value::Array(required)) = schema.get("required") {
        for req in required {
            if let Some(key) = req.as_str()
                && !obj.contains_key(key)
            {
                return Err(format!("{path}: missing required property `{key}`"));
            }
        }
    }
    let props = schema.get("properties").and_then(Value::as_object);
    if let Some(prop_map) = props {
        for (key, prop_schema) in prop_map {
            if let Some(actual) = obj.get(key)
                && let Some(sub) = prop_schema.as_object()
            {
                validate_value_with_defs(
                    sub,
                    defs,
                    actual,
                    &format!("{path}.{key}"),
                    depth + 1,
                    strict_excess,
                )?;
            }
        }
    }
    match schema.get("additionalProperties") {
        Some(Value::Bool(false)) => {
            for key in obj.keys() {
                if !props.is_some_and(|p| p.contains_key(key)) {
                    return Err(format!("{path}: unexpected property `{key}`"));
                }
            }
        }
        Some(Value::Object(extra_schema)) => {
            for (key, actual) in obj {
                if !props.is_some_and(|p| p.contains_key(key)) {
                    validate_value_with_defs(
                        extra_schema,
                        defs,
                        actual,
                        &format!("{path}.{key}"),
                        depth + 1,
                        strict_excess,
                    )?;
                }
            }
        }
        None if strict_excess && props.is_some() => {
            for key in obj.keys() {
                if !props.is_some_and(|p| p.contains_key(key)) {
                    return Err(format!("{path}: unexpected property `{key}`"));
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn validate_array(
    schema: &Map<String, Value>,
    defs: Option<&Map<String, Value>>,
    arr: &[Value],
    path: &str,
    depth: usize,
    strict_excess: bool,
) -> Result<(), String> {
    if let Some(min) = schema.get("minItems").and_then(Value::as_u64)
        && (arr.len() as u64) < min
    {
        return Err(format!("{path}: expected at least {min} items"));
    }
    if let Some(max) = schema.get("maxItems").and_then(Value::as_u64)
        && (arr.len() as u64) > max
    {
        return Err(format!("{path}: expected at most {max} items"));
    }
    if let Some(Value::Object(item_schema)) = schema.get("items") {
        for (idx, item) in arr.iter().enumerate() {
            validate_value_with_defs(
                item_schema,
                defs,
                item,
                &format!("{path}[{idx}]"),
                depth + 1,
                strict_excess,
            )?;
        }
    }
    Ok(())
}

fn validate_string(schema: &Map<String, Value>, s: &str, path: &str) -> Result<(), String> {
    let char_len = s.chars().count() as u64;
    if let Some(min) = schema.get("minLength").and_then(Value::as_u64)
        && char_len < min
    {
        return Err(format!("{path}: string shorter than minLength {min}"));
    }
    if let Some(max) = schema.get("maxLength").and_then(Value::as_u64)
        && char_len > max
    {
        return Err(format!("{path}: string longer than maxLength {max}"));
    }
    Ok(())
}

fn validate_number(
    schema: &Map<String, Value>,
    num: &serde_json::Number,
    path: &str,
) -> Result<(), String> {
    let Some(val) = num.as_f64() else {
        return Ok(());
    };
    if let Some(min) = schema.get("minimum").and_then(Value::as_f64)
        && val < min
    {
        return Err(format!("{path}: {val} < minimum {min}"));
    }
    if let Some(max) = schema.get("maximum").and_then(Value::as_f64)
        && val > max
    {
        return Err(format!("{path}: {val} > maximum {max}"));
    }
    Ok(())
}

fn matches_schema_type(ty: &Value, value: &Value) -> bool {
    match ty {
        Value::String(name) => matches_single_type(name, value),
        Value::Array(variants) => variants
            .iter()
            .any(|v| v.as_str().is_some_and(|n| matches_single_type(n, value))),
        _ => false,
    }
}

fn matches_single_type(ty: &str, value: &Value) -> bool {
    match ty {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        "number" => value.is_number(),
        "integer" => {
            value.as_i64().is_some()
                || value.as_u64().is_some()
                || value
                    .as_f64()
                    .is_some_and(|f| f.is_finite() && f.fract() == 0.0)
        }
        _ => false,
    }
}
