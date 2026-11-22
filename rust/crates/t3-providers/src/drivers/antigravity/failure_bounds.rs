//! Antigravity turn error classification (mid-turn retryable 404) and Unicode-safe command bounding.

use std::sync::OnceLock;

use regex::Regex;
use serde_json::{Map, Value};
use t3_shared::utf16::{utf8_len_to_utf16, utf16_slice_safe};

const TOOL_TEXT_TRUNCATED: &str = "... [truncated]\n";
const DEFAULT_TOOL_TEXT_LIMIT: usize = 8_000;

const COMMAND_TEXT_FIELDS: &[&str] = &[
    "CommandLine",
    "command_line",
    "commandLine",
    "command",
    "aggregatedOutput",
    "combinedOutput",
    "combined_output",
    "output",
    "text",
];

/// Classified failure details from an Antigravity turn error step (`f68d36d8aa`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AntigravityTurnFailure {
    /// `"usage_limit"` or `"provider_error"`.
    pub class: &'static str,
    /// Human-readable API/provider error message.
    pub message: String,
    /// Optional status code string (e.g. `"NOT_FOUND"`, `"RESOURCE_EXHAUSTED"`).
    pub code: Option<String>,
    /// Whether retrying immediately is safe.
    pub retryable: bool,
    /// `"reusable"` or `"broken"`.
    pub thread_disposition: &'static str,
    /// Automatic continuation prompt when mid-turn transient 404 struck (`"continue"`).
    pub resume_with: Option<&'static str>,
}

fn api_error_regex() -> Option<&'static Regex> {
    static RE: OnceLock<Option<Regex>> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?s)Error (\d{3}), Message: (.+?), Status: ([A-Z_]+)").ok())
        .as_ref()
}

fn status_error_regex() -> Option<&'static Regex> {
    static RE: OnceLock<Option<Regex>> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?:^|[^A-Z_])([A-Z_]+) \(code (\d{3})\): ([^\x00-\x1F\x7F]+)").ok()
    })
    .as_ref()
}

/// Classifies an Antigravity error payload (`Error 404...` or `STATUS (code 404): ...`),
/// marking mid-turn 404 errors as retryable with `resume_with = Some("continue")` when
/// `reached_model` is `true` (`f68d36d8aa`).
#[must_use]
pub fn classify_antigravity_turn_failure(
    error_text: &str,
    reached_model: bool,
) -> Option<AntigravityTurnFailure> {
    let (http_status, message, status_name) =
        if let Some(caps) = api_error_regex().and_then(|re| re.captures(error_text)) {
            (
                caps.get(1)?.as_str().to_owned(),
                caps.get(2)?.as_str().trim().to_owned(),
                caps.get(3)?.as_str().to_owned(),
            )
        } else {
            let caps = status_error_regex().and_then(|re| re.captures(error_text))?;
            (
                caps.get(2)?.as_str().to_owned(),
                caps.get(3)?.as_str().trim().to_owned(),
                caps.get(1)?.as_str().to_owned(),
            )
        };

    let rate_limited = http_status == "429" || status_name == "RESOURCE_EXHAUSTED";
    let status_num: u16 = http_status.parse().unwrap_or(0);
    let transient_not_found = http_status == "404" && reached_model;
    Some(AntigravityTurnFailure {
        class: if rate_limited {
            "usage_limit"
        } else {
            "provider_error"
        },
        message,
        code: Some(status_name),
        retryable: rate_limited || transient_not_found || status_num >= 500,
        thread_disposition: "reusable",
        resume_with: transient_not_found.then_some("continue"),
    })
}

/// Keeps the valid Unicode tail of long command/output text up to `limit` UTF-16 units (`df09ced768`).
#[must_use]
pub fn bound_antigravity_command_text(text: &str, limit: usize) -> String {
    let effective_limit = limit.max(1);
    let prefix_u16 = utf8_len_to_utf16(TOOL_TEXT_TRUNCATED);
    if utf8_len_to_utf16(text) <= effective_limit
        || (utf8_len_to_utf16(text) <= effective_limit + prefix_u16
            && text.starts_with(TOOL_TEXT_TRUNCATED))
    {
        return text.to_owned();
    }
    let total_u16 = utf8_len_to_utf16(text);
    let start_u16 = total_u16.saturating_sub(effective_limit);
    let tail = utf16_slice_safe(text, start_u16, total_u16);
    format!("{TOOL_TEXT_TRUNCATED}{tail}")
}

/// Bounds any command or output fields inside an Antigravity tool JSON object using
/// Unicode-safe tail truncation (`df09ced768`).
#[must_use]
pub fn sanitize_antigravity_command_fields(obj: &Map<String, Value>) -> Map<String, Value> {
    obj.iter()
        .map(|(key, val)| {
            let next = match val {
                Value::String(s) if COMMAND_TEXT_FIELDS.contains(&key.as_str()) => {
                    Value::String(bound_antigravity_command_text(s, DEFAULT_TOOL_TEXT_LIMIT))
                }
                other => other.clone(),
            };
            (key.clone(), next)
        })
        .collect()
}
