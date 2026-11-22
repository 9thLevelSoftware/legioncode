//! OTLP JSON trace ingestor (`POST /api/observability/v1/traces`) writing NDJSON for `t3 trace summary`.

use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::{Method, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};
use t3_protocol::url_redaction::redact_http_request_target;

use crate::error::ServerRuntimeError;
use crate::http_api::RuntimeHttpState;

/// Formats a single HTTP access log line with signed paths and sensitive query tokens redacted (`#40133`).
pub fn format_redacted_access_log(
    method: &Method,
    request_target: &str,
    status: StatusCode,
) -> String {
    let safe_target = redact_http_request_target(request_target);
    format!("{} {} -> {}", method.as_str(), safe_target, status.as_u16())
}

/// Axum middleware recording redacted HTTP access log entries onto [`RuntimeHttpState::access_logs`] (`#40133`).
pub(crate) async fn redacted_access_log_middleware(
    State(state): State<Arc<RuntimeHttpState>>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let method = req.method().clone();
    let target = req
        .uri()
        .path_and_query()
        .map_or_else(|| req.uri().path().to_owned(), ToString::to_string);
    let resp = next.run(req).await;
    let line = format_redacted_access_log(&method, &target, resp.status());
    state.record_access_log(line);
    resp
}

fn redact_span_or_url_text(raw: &str) -> String {
    if let Some((verb, rest)) = raw.split_once(' ')
        && matches!(
            verb,
            "GET" | "POST" | "PUT" | "DELETE" | "PATCH" | "HEAD" | "OPTIONS"
        )
        && (rest.starts_with('/') || rest.starts_with("http://") || rest.starts_with("https://"))
    {
        return format!("{verb} {}", redact_http_request_target(rest));
    }
    redact_http_request_target(raw)
}

fn redact_trace_value(value: Value) -> Value {
    match value {
        Value::String(s) => Value::String(redact_span_or_url_text(&s)),
        Value::Array(items) => Value::Array(items.into_iter().map(redact_trace_value).collect()),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| (k, redact_trace_value(v)))
                .collect(),
        ),
        other => other,
    }
}

/// `POST /api/observability/v1/traces` — normalize OTLP JSON spans into `<logs_dir>/traces.ndjson`.
pub(crate) async fn handle_otlp_traces(
    State(state): State<Arc<RuntimeHttpState>>,
    Json(payload): Json<Value>,
) -> Result<Response, ServerRuntimeError> {
    let lines = normalize_otlp_payload(&payload)?;
    let trace_path = state.traces_file_path();
    let write_lock = state.trace_write_lock();

    tokio::task::spawn_blocking(move || -> Result<(), ServerRuntimeError> {
        if let Some(parent) = trace_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let _guard = write_lock
            .lock()
            .map_err(|_| ServerRuntimeError::Persistence("trace write lock poisoned".to_owned()))?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&trace_path)?;
        for line in &lines {
            writeln!(file, "{line}")?;
        }
        file.flush()?;
        Ok(())
    })
    .await
    .map_err(|e| ServerRuntimeError::Process(format!("trace write task failed: {e}")))??;

    Ok((StatusCode::OK, Json(json!({ "partialSuccess": {} }))).into_response())
}

/// Convert an OTLP `ExportTraceServiceRequest` JSON payload into NDJSON lines compatible with `t3_remote::trace`.
pub fn normalize_otlp_payload(payload: &Value) -> Result<Vec<String>, ServerRuntimeError> {
    let Some(resource_spans) = payload.get("resourceSpans").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    let mut lines = Vec::new();
    for resource_span in resource_spans {
        let resource_attrs = resource_span
            .get("resource")
            .and_then(|r| r.get("attributes"))
            .map_or_default(flatten_attributes);
        let Some(scope_spans) = resource_span.get("scopeSpans").and_then(Value::as_array) else {
            continue;
        };
        for scope_span in scope_spans {
            let Some(spans) = scope_span.get("spans").and_then(Value::as_array) else {
                continue;
            };
            for span in spans {
                if let Some(record) = normalize_span(span, &resource_attrs) {
                    lines.push(serde_json::to_string(&record)?);
                }
            }
        }
    }
    Ok(lines)
}

fn normalize_span(span: &Value, resource_attrs: &Map<String, Value>) -> Option<Value> {
    let name = span.get("name").and_then(Value::as_str)?.trim();
    if name.is_empty() {
        return None;
    }
    let start_ns = parse_nano_u128(span.get("startTimeUnixNano"))?;
    let end_ns = parse_nano_u128(span.get("endTimeUnixNano")).unwrap_or(start_ns);
    let elapsed_ns = end_ns.saturating_sub(start_ns);
    let duration_ms = nanos_to_duration_ms(elapsed_ns);

    let mut attrs = Map::new();
    for (k, v) in resource_attrs {
        attrs.insert(k.clone(), redact_trace_value(v.clone()));
    }
    if let Some(span_attrs) = span.get("attributes") {
        for (k, v) in flatten_attributes(span_attrs) {
            attrs.insert(k, redact_trace_value(v));
        }
    }

    let safe_name = redact_span_or_url_text(name);
    let mut out = Map::new();
    out.insert("name".to_owned(), Value::String(safe_name));
    out.insert(
        "startTimeUnixNano".to_owned(),
        Value::String(start_ns.to_string()),
    );
    out.insert(
        "endTimeUnixNano".to_owned(),
        Value::String(end_ns.to_string()),
    );
    out.insert("durationMs".to_owned(), json!(duration_ms));
    if let Some(trace_id) = span.get("traceId").and_then(Value::as_str) {
        out.insert("traceId".to_owned(), Value::String(trace_id.to_owned()));
    }
    if let Some(span_id) = span.get("spanId").and_then(Value::as_str) {
        out.insert("spanId".to_owned(), Value::String(span_id.to_owned()));
    }
    if let Some(parent_id) = span.get("parentSpanId").and_then(Value::as_str)
        && !parent_id.is_empty()
    {
        out.insert(
            "parentSpanId".to_owned(),
            Value::String(parent_id.to_owned()),
        );
    }
    if let Some(status) = normalize_status(span.get("status")) {
        out.insert("status".to_owned(), status);
    }
    out.insert("attributes".to_owned(), Value::Object(attrs));
    Some(Value::Object(out))
}

fn nanos_to_duration_ms(elapsed_ns: u128) -> f64 {
    let whole_ms = u32::try_from(elapsed_ns / 1_000_000).unwrap_or(u32::MAX);
    let sub_ns = u32::try_from(elapsed_ns % 1_000_000).unwrap_or(0);
    f64::from(whole_ms) + f64::from(sub_ns) / 1_000_000.0
}

fn parse_nano_u128(val: Option<&Value>) -> Option<u128> {
    match val? {
        Value::String(s) => s.trim().parse::<u128>().ok(),
        Value::Number(n) => n.as_u64().map(u128::from),
        _ => None,
    }
}

fn normalize_status(raw: Option<&Value>) -> Option<Value> {
    let obj = raw?.as_object()?;
    let code_str = match obj.get("code") {
        Some(Value::Number(n)) => match n.as_u64() {
            Some(2) => "2",
            Some(1) => "1",
            _ => "0",
        },
        Some(Value::String(s)) => match s.as_str() {
            "2" | "STATUS_CODE_ERROR" | "ERROR" => "2",
            "1" | "STATUS_CODE_OK" | "OK" => "1",
            _ => "0",
        },
        _ => "0",
    };
    let mut out = Map::new();
    out.insert("code".to_owned(), Value::String(code_str.to_owned()));
    if let Some(msg) = obj.get("message").and_then(Value::as_str)
        && !msg.is_empty()
    {
        out.insert("message".to_owned(), Value::String(msg.to_owned()));
    }
    Some(Value::Object(out))
}

fn flatten_attributes(raw: &Value) -> Map<String, Value> {
    let mut out = Map::new();
    if let Some(arr) = raw.as_array() {
        for item in arr {
            let Some(key) = item.get("key").and_then(Value::as_str) else {
                continue;
            };
            let Some(val_obj) = item.get("value").and_then(Value::as_object) else {
                continue;
            };
            if let Some(decoded) = unwrap_otlp_any_value(val_obj) {
                out.insert(key.to_owned(), decoded);
            }
        }
    } else if let Some(map) = raw.as_object() {
        for (k, v) in map {
            out.insert(k.clone(), v.clone());
        }
    }
    out
}

fn unwrap_otlp_any_value(val: &Map<String, Value>) -> Option<Value> {
    if let Some(v) = val.get("stringValue") {
        return Some(v.clone());
    }
    if let Some(v) = val.get("boolValue") {
        return Some(v.clone());
    }
    if let Some(v) = val.get("doubleValue") {
        return Some(v.clone());
    }
    if let Some(v) = val.get("intValue") {
        if let Some(s) = v.as_str()
            && let Ok(n) = s.parse::<i64>()
        {
            return Some(json!(n));
        }
        return Some(v.clone());
    }
    val.values().next().cloned()
}
