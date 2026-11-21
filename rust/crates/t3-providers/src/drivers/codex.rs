//! Codex stdio MCP env injection, launch arg overrides, web search query
//! deduplication, and encrypted history rejection recovery.

use std::collections::{BTreeMap, HashSet};

use serde_json::Value;
use t3_contracts::brand::{ACTIVE_BRAND, BrandConfig};
use t3_contracts::orchestration::ProviderFailureClass;
use t3_protocol::provider_env::SanitizedEnv;

pub use super::codex_probe::{
    CodexBinaryFingerprint, CodexLocalBinaryKind, CodexLocalProbeCache, CodexLocalProbeRecord,
    inspect_codex_binary_fingerprint, parse_codex_cli_version,
};

/// Managed failure code reported by Codex when encrypted conversation history fails verification.
pub const CODEX_INVALID_ENCRYPTED_CONTENT_CODE: &str = "invalid_encrypted_content";

/// Sanitized user-facing recovery message for rejected encrypted Codex history.
pub const CODEX_INVALID_ENCRYPTED_HISTORY_MESSAGE: &str = "Codex could not verify this session's encrypted conversation history. Send another message to continue in a fresh session with the visible conversation history.";

/// Returns stdio MCP server names from a `config/read` `mcp_servers` map, excluding dotted keys.
#[must_use]
pub fn codex_stdio_mcp_server_names(mcp_servers: Option<&Value>) -> Vec<String> {
    let Some(map) = mcp_servers.and_then(Value::as_object) else {
        return Vec::new();
    };
    map.iter()
        .filter(|(name, server)| {
            !name.contains('.')
                && server
                    .as_object()
                    .and_then(|obj| obj.get("command"))
                    .is_some_and(Value::is_string)
        })
        .map(|(name, _)| name.clone())
        .collect()
}

/// Returns `<BRAND>_THREAD_ID` environment variable name (`T3CODE_THREAD_ID`, `LEGION_THREAD_ID`, etc.).
#[must_use]
pub fn codex_thread_env_name(brand: Option<BrandConfig>) -> String {
    let brand = brand.unwrap_or(ACTIVE_BRAND);
    format!("{}THREAD_ID", brand.primary_env_prefix)
}

/// Builds per-thread Codex config overrides injecting `<BRAND>_THREAD_ID` into shell env and stdio MCP servers.
#[must_use]
pub fn codex_thread_runtime_config_overrides(
    thread_id: Option<&str>,
    stdio_mcp_servers: &[String],
    brand: Option<BrandConfig>,
) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    let Some(tid) = thread_id.filter(|s| !s.trim().is_empty()) else {
        return out;
    };
    let env_name = codex_thread_env_name(brand);
    out.insert(
        format!("shell_environment_policy.set.{env_name}"),
        Value::String(tid.to_owned()),
    );
    for server in stdio_mcp_servers {
        if !server.is_empty() && !server.contains('.') {
            out.insert(
                format!("mcp_servers.{server}.env.{env_name}"),
                Value::String(tid.to_owned()),
            );
        }
    }
    out
}

/// Resolves Codex launch args using an explicit lookup (such as a host env map)
/// without stripping escaped trailing spaces (`\ `) from non-blank overrides.
pub fn resolve_codex_launch_args_with_lookup(
    configured_launch_args: Option<&str>,
    lookup: impl FnMut(&str) -> Option<String>,
) -> String {
    if let Some(override_raw) = ACTIVE_BRAND.read_env_from("CODEX_LAUNCH_ARGS", lookup) {
        return override_raw;
    }
    configured_launch_args.map_or("", str::trim).to_owned()
}

/// Resolves Codex launch args from a [`SanitizedEnv`] without stripping escaped trailing spaces (`\ `).
#[must_use]
pub fn resolve_codex_launch_args(
    configured_launch_args: Option<&str>,
    env: &SanitizedEnv,
) -> String {
    resolve_codex_launch_args_with_lookup(configured_launch_args, |key| {
        env.get(key).map(str::to_owned)
    })
}

/// Splits CLI launch args honoring quotes and backslash escapes including `\ ` trailing spaces.
#[must_use]
pub fn tokenize_cli_args(raw: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_token = false;
    let mut quote: Option<char> = None;
    let mut chars = raw.chars();

    while let Some(ch) = chars.next() {
        match quote {
            Some(q) => {
                if ch == q {
                    quote = None;
                } else if ch == '\\' && q == '"' {
                    if let Some(escaped) = chars.next() {
                        current.push(escaped);
                    }
                } else {
                    current.push(ch);
                }
            }
            None => match ch {
                '\'' | '"' => {
                    in_token = true;
                    quote = Some(ch);
                }
                '\\' => {
                    in_token = true;
                    if let Some(escaped) = chars.next() {
                        current.push(escaped);
                    }
                }
                c if c.is_whitespace() => {
                    if in_token {
                        args.push(std::mem::take(&mut current));
                        in_token = false;
                    }
                }
                other => {
                    in_token = true;
                    current.push(other);
                }
            },
        }
    }
    if in_token {
        args.push(current);
    }
    args
}

/// Trims and deduplicates web search query values while preserving first-seen order.
#[must_use]
pub fn dedupe_web_search_queries<'a>(
    values: impl IntoIterator<Item = Option<&'a str>>,
) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for value in values.into_iter().flatten() {
        let trimmed = value.trim();
        if !trimmed.is_empty() && seen.insert(trimmed.to_owned()) {
            out.push(trimmed.to_owned());
        }
    }
    out
}

/// Sanitized structured failure extracted from Codex error payloads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexManagedFailure {
    /// Extracted inner error code (for example `invalid_encrypted_content`).
    pub code: String,
    /// Failure classification compatible with `ProviderFailureClass`.
    pub class: ProviderFailureClass,
    /// Redacted user-facing explanation (without upstream sensitive headers).
    pub message: String,
}

/// Classifies Codex structured error envelopes, specifically handling `invalid_encrypted_content`.
#[must_use]
pub fn classify_codex_managed_error(raw_message: &str) -> Option<CodexManagedFailure> {
    if let Ok(Value::Object(root)) = serde_json::from_str::<Value>(raw_message) {
        let code = root
            .get("error")
            .and_then(Value::as_object)
            .and_then(|err| err.get("code"))
            .and_then(Value::as_str)
            .or_else(|| root.get("code").and_then(Value::as_str));
        if code == Some(CODEX_INVALID_ENCRYPTED_CONTENT_CODE) {
            return Some(CodexManagedFailure {
                code: CODEX_INVALID_ENCRYPTED_CONTENT_CODE.to_owned(),
                class: ProviderFailureClass::ProviderError,
                message: CODEX_INVALID_ENCRYPTED_HISTORY_MESSAGE.to_owned(),
            });
        }
    }
    if raw_message.contains(CODEX_INVALID_ENCRYPTED_CONTENT_CODE) {
        return Some(CodexManagedFailure {
            code: CODEX_INVALID_ENCRYPTED_CONTENT_CODE.to_owned(),
            class: ProviderFailureClass::ProviderError,
            message: CODEX_INVALID_ENCRYPTED_HISTORY_MESSAGE.to_owned(),
        });
    }
    None
}

/// Decision returned before resuming a Codex thread whose previous turn failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexHistoryRecoveryDecision {
    /// Proceed with normal native resume.
    ContinueNormalResume,
    /// Start a fresh native session with projected conversation history injected.
    RecoverInFreshSession,
    /// Pending background work/subagents still run on the native thread; fail until settled.
    BlockedByPendingBackgroundWork,
}

/// Evaluates whether a Codex thread should rotate into a fresh session after encrypted history rejection.
#[must_use]
pub fn codex_should_recover_rejected_history(
    error_code: Option<&str>,
    raw_error_message: &str,
    native_thread_already_rotated: bool,
    has_pending_background_work: bool,
) -> CodexHistoryRecoveryDecision {
    if native_thread_already_rotated {
        return CodexHistoryRecoveryDecision::ContinueNormalResume;
    }
    let is_rejected_history = error_code == Some(CODEX_INVALID_ENCRYPTED_CONTENT_CODE)
        || classify_codex_managed_error(raw_error_message)
            .is_some_and(|m| m.code == CODEX_INVALID_ENCRYPTED_CONTENT_CODE);
    if !is_rejected_history {
        return CodexHistoryRecoveryDecision::ContinueNormalResume;
    }
    if has_pending_background_work {
        CodexHistoryRecoveryDecision::BlockedByPendingBackgroundWork
    } else {
        CodexHistoryRecoveryDecision::RecoverInFreshSession
    }
}
