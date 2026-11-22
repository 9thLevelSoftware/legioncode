//! JSON Schema structural checks and value validation for [`super::output_ops`].

use legion_contracts::orchestration::RunOutputSchema;
use serde_json::{Map, Value};

const MAX_SCHEMA_DEPTH: usize = 32;

/// Checks basic structural validity of a declared `outputSchema`.
pub(super) fn validate_schema_shape(
    schema: &Map<String, Value>,
    depth: usize,
) -> Result<(), String> {
    if depth > MAX_SCHEMA_DEPTH {
        return Err("The output schema exceeds maximum nesting depth.".to_owned());
    }
    if let Some(ty) = schema.get("type") {
        let valid_type = match ty {
            Value::String(s) => is_known_json_type(s),
            Value::Array(arr) => {
                !arr.is_empty()
                    && arr
                        .iter()
                        .all(|v| v.as_str().is_some_and(is_known_json_type))
            }
            _ => false,
        };
        if !valid_type {
            return Err("The output schema is not a usable JSON Schema.".to_owned());
        }
    }
    if let Some(req) = schema.get("required") {
        let valid_req = req
            .as_array()
            .is_some_and(|arr| arr.iter().all(Value::is_string));
        if !valid_req {
            return Err("The output schema is not a usable JSON Schema.".to_owned());
        }
    }
    if let Some(props) = schema.get("properties") {
        let Some(map) = props.as_object() else {
            return Err("The output schema is not a usable JSON Schema.".to_owned());
        };
        for child in map.values() {
            if let Some(child_map) = child.as_object() {
                validate_schema_shape(child_map, depth + 1)?;
            } else {
                return Err("The output schema is not a usable JSON Schema.".to_owned());
            }
        }
    }
    Ok(())
}

/// Validates a submitted JSON `value` against `schema` (`type`, `required`, `properties`,
/// `additionalProperties`, `items`, `enum`, `const`, and numeric/length constraints).
pub fn validate_output_schema_value(schema: &RunOutputSchema, value: &Value) -> Result<(), String> {
    validate_value_against_map(schema, value, "$", 0)
}

fn is_known_json_type(ty: &str) -> bool {
    matches!(
        ty,
        "object" | "array" | "string" | "integer" | "number" | "boolean" | "null"
    )
}

fn validate_value_against_map(
    schema: &Map<String, Value>,
    value: &Value,
    path: &str,
    depth: usize,
) -> Result<(), String> {
    if depth > MAX_SCHEMA_DEPTH {
        return Err(format!("{path}: exceeded maximum nesting depth"));
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
        Value::Object(obj) => validate_object_constraints(schema, obj, path, depth),
        Value::Array(arr) => validate_array_constraints(schema, arr, path, depth),
        Value::String(s) => validate_string_constraints(schema, s, path),
        Value::Number(num) => validate_number_constraints(schema, num, path),
        _ => Ok(()),
    }
}

fn validate_object_constraints(
    schema: &Map<String, Value>,
    obj: &Map<String, Value>,
    path: &str,
    depth: usize,
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
                validate_value_against_map(sub, actual, &format!("{path}.{key}"), depth + 1)?;
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
                    validate_value_against_map(
                        extra_schema,
                        actual,
                        &format!("{path}.{key}"),
                        depth + 1,
                    )?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn validate_array_constraints(
    schema: &Map<String, Value>,
    arr: &[Value],
    path: &str,
    depth: usize,
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
            validate_value_against_map(item_schema, item, &format!("{path}[{idx}]"), depth + 1)?;
        }
    }
    Ok(())
}

fn validate_string_constraints(
    schema: &Map<String, Value>,
    s: &str,
    path: &str,
) -> Result<(), String> {
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

fn validate_number_constraints(
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
        Value::Array(variants) => variants.iter().any(|v| {
            v.as_str()
                .is_some_and(|name| matches_single_type(name, value))
        }),
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
