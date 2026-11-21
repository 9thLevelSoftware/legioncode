//! NDJSON OTLP span line parsing and JS-compatible nanosecond timestamp validation.

use serde_json::Value;

const MAX_JS_DATE_MS: f64 = 8_640_000_000_000_000.0;
const MAX_JS_DATE_MS_I128: i128 = 8_640_000_000_000_000;

pub(crate) struct ParsedSpanLine {
    pub(crate) name: String,
    pub(crate) duration_ms: f64,
    pub(crate) end_ms: i64,
    pub(crate) interrupted: bool,
    pub(crate) failed: bool,
}

pub(crate) fn parse_trace_span_line(line: &str) -> Option<ParsedSpanLine> {
    let value: Value = serde_json::from_str(line).ok()?;
    let obj = value.as_object()?;

    let name = obj.get("name")?.as_str()?.to_owned();
    let duration_ms = obj.get("durationMs")?.as_f64()?;
    if !duration_ms.is_finite() {
        return None;
    }

    let end_ns_str = obj.get("endTimeUnixNano")?.as_str()?;
    let end_ms = parse_unix_nano_to_ms(end_ns_str)?;

    let exit_tag = match obj.get("exit") {
        None => None,
        Some(raw_exit) => {
            let exit_obj = raw_exit.as_object()?;
            Some(exit_obj.get("_tag")?.as_str()?)
        }
    };

    let (status_code, status_message) = match obj.get("status") {
        None => (None, None),
        Some(raw_status) => {
            let status_obj = raw_status.as_object()?;
            let code = match status_obj.get("code") {
                None => None,
                Some(raw_code) => Some(raw_code.as_str()?),
            };
            let message = match status_obj.get("message") {
                None => None,
                Some(raw_msg) => Some(raw_msg.as_str()?),
            };
            (code, message)
        }
    };

    let attr_interrupted = match obj.get("attributes") {
        None => false,
        Some(raw_attrs) => {
            let attrs_obj = raw_attrs.as_object()?;
            attrs_obj.get("effect.fiber.interrupted") == Some(&Value::Bool(true))
        }
    };

    let interrupted = exit_tag == Some("Interrupted")
        || attr_interrupted
        || status_message == Some("Interrupted");
    let failed = exit_tag == Some("Failure") || status_code == Some("2");

    Some(ParsedSpanLine {
        name,
        duration_ms,
        end_ms,
        interrupted,
        failed,
    })
}

fn parse_unix_nano_to_ms(raw: &str) -> Option<i64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(ns) = trimmed.parse::<i128>() {
        let ms = ns / 1_000_000;
        if ms.abs() > MAX_JS_DATE_MS_I128 {
            return None;
        }
        return i64::try_from(ms).ok();
    }
    let ns_f64 = trimmed.parse::<f64>().ok()?;
    if !ns_f64.is_finite() {
        return None;
    }
    let end_ms = ns_f64 / 1_000_000.0;
    if !end_ms.is_finite() || end_ms.abs() > MAX_JS_DATE_MS {
        return None;
    }
    let trunc = if end_ms == 0.0 { 0.0 } else { end_ms.trunc() };
    let text = format!("{trunc:.0}");
    let normalized = if text == "-0" { "0" } else { text.as_str() };
    normalized.parse::<i64>().ok()
}
