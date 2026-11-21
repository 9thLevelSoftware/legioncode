//! Provider failure classification, secret redaction, and macOS/HTTP guidance.
//!
//! Mirrors `packages/provider-core/src/server/failure.ts` and re-exports
//! usage-limit reconciliation helpers from [`super::usage_limits`].

use std::sync::OnceLock;

use regex::Regex;
use t3_contracts::orchestration::{ProviderFailure, ProviderFailureClass};
use t3_shared::utf16::{utf8_len_to_utf16, utf16_slice_safe};

pub use super::usage_limits::{
    ProbeAvailability, UsageLimitsSnapshot, UsageWindow, UsageWindowKind,
    apply_usage_limits_update, clamp_percent, normalize_usage_windows,
    resolve_usage_limits_after_probe,
};

/// Maximum characters preserved in a classified failure message.
pub const MAX_PROVIDER_FAILURE_MESSAGE_LENGTH: usize = 4_096;
/// Maximum characters preserved in a normalized failure error code.
pub const MAX_PROVIDER_FAILURE_CODE_LENGTH: usize = 128;
/// Default retry budget ceiling (`10 minutes`) for retryable provider failures.
pub const PROVIDER_RETRY_BUDGET_MS: u64 = 600_000;

struct SecretPatterns {
    url_creds: Regex,
    auth_header: Regex,
    api_tokens: Regex,
    kv_secrets: Regex,
}

fn secret_patterns() -> &'static SecretPatterns {
    static PATTERNS: OnceLock<SecretPatterns> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        let url_creds = Regex::new(r"(https?://)[^:/\s@]+:[^@/\s]+@");
        // INFALLIBLE: static constant regex validated against literal syntax.
        let url_creds = url_creds.expect("valid URL credential redaction regex");

        let auth_header = Regex::new(r"(?i)\b(Bearer|Basic)\s+[A-Za-z0-9._~+/=-]{6,}");
        // INFALLIBLE: static constant regex validated against literal syntax.
        let auth_header = auth_header.expect("valid HTTP Authorization header redaction regex");

        let api_tokens = Regex::new(
            r"\b(?:sk-[A-Za-z0-9_-]{8,}|xai-[A-Za-z0-9_-]{8,}|AIza[A-Za-z0-9_-]{12,}|ghp_[A-Za-z0-9]{12,})\b",
        );
        // INFALLIBLE: static constant regex validated against literal syntax.
        let api_tokens = api_tokens.expect("valid API token redaction regex");

        let kv_secrets = Regex::new(
            r#"(?i)\b(api[_-]?key|access[_-]?token|refresh[_-]?token|password|secret)\s*([=:])\s*[^\s,"';]+"#,
        );
        // INFALLIBLE: static constant regex validated against literal syntax.
        let kv_secrets = kv_secrets.expect("valid key-value secret redaction regex");

        SecretPatterns {
            url_creds,
            auth_header,
            api_tokens,
            kv_secrets,
        }
    })
}

/// Redacts URL userinfo, HTTP Bearer/Basic tokens, common provider API keys,
/// and secret `key=value` fragments, then bounds output to
/// [`MAX_PROVIDER_FAILURE_MESSAGE_LENGTH`] UTF-16 code units.
///
/// # Examples
///
/// ```
/// use t3_providers::core::redact_sensitive_text;
///
/// let msg = "POST https://user:pw@api.example.com failed with Bearer secret-token-12345 and sk-abc123456789";
/// let clean = redact_sensitive_text(msg);
/// assert!(!clean.contains("pw"));
/// assert!(!clean.contains("secret-token-12345"));
/// assert!(!clean.contains("sk-abc123456789"));
/// assert!(clean.contains("[REDACTED]"));
/// ```
#[must_use]
pub fn redact_sensitive_text(raw: &str) -> String {
    let patterns = secret_patterns();
    let step1 = patterns.url_creds.replace_all(raw, "${1}[REDACTED]@");
    let step2 = patterns.auth_header.replace_all(&step1, "$1 [REDACTED]");
    let step3 = patterns.api_tokens.replace_all(&step2, "[REDACTED_KEY]");
    let step4 = patterns.kv_secrets.replace_all(&step3, "$1$2[REDACTED]");
    truncate_bounded(&step4, MAX_PROVIDER_FAILURE_MESSAGE_LENGTH)
}

/// Truncates `raw` safely at UTF-16 code-unit boundary `max_len`.
#[must_use]
pub fn truncate_bounded(raw: &str, max_len: usize) -> String {
    let trimmed = raw.trim();
    if utf8_len_to_utf16(trimmed) <= max_len {
        trimmed.to_owned()
    } else {
        utf16_slice_safe(trimmed, 0, max_len).trim_end().to_owned()
    }
}

/// Structured diagnostic signal passed to [`classify_provider_failure`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderFailureInput {
    /// Raw stderr, RPC error message, or transport exception text.
    pub message: String,
    /// Optional provider or OS error code string.
    pub code: Option<String>,
    /// Optional HTTP response status code.
    pub http_status: Option<u16>,
    /// Optional JSON-RPC numeric error code.
    pub json_rpc_code: Option<i64>,
    /// Optional POSIX/Win32 errno symbol (`"EPERM"`, `"ENOENT"`, etc.).
    pub os_error_code: Option<String>,
    /// Whether the environment runs on macOS (enables TCC/Full Disk Access hints).
    pub is_macos: bool,
}

/// Classified, redacted failure output convertible into [`ProviderFailure`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassifiedProviderFailure {
    /// Canonical failure taxonomy class.
    pub class: ProviderFailureClass,
    /// Bounded machine-readable error code if any.
    pub code: Option<String>,
    /// Redacted human-readable failure explanation.
    pub message: String,
    /// Whether automated retry is safe/permitted.
    pub retryable: bool,
    /// Optional cumulative retry time budget in milliseconds.
    pub retry_budget_ms: Option<u64>,
}

impl ClassifiedProviderFailure {
    /// Converts this classification into the wire-contract [`ProviderFailure`].
    #[must_use]
    pub fn to_contract(&self) -> ProviderFailure {
        ProviderFailure {
            class: self.class,
            message: self.message.clone(),
            code: self.code.clone(),
            retryable: Some(self.retryable),
            reset_at: None,
        }
    }
}

/// Classifies raw provider error telemetry into a redacted, retry-aware
/// [`ClassifiedProviderFailure`].
#[must_use]
pub fn classify_provider_failure(input: &ProviderFailureInput) -> ClassifiedProviderFailure {
    let base_raw = if input.message.trim().is_empty() {
        "Provider operation failed"
    } else {
        input.message.as_str()
    };
    let lower = base_raw.to_ascii_lowercase();
    let code_norm = input
        .code
        .as_deref()
        .map(|c| truncate_bounded(c, MAX_PROVIDER_FAILURE_CODE_LENGTH))
        .filter(|c| !c.is_empty());

    if let Some(specific) = classify_specific_failure(input, base_raw, &lower, code_norm.clone()) {
        return specific;
    }

    if input.json_rpc_code == Some(-32_000) {
        let detail = redact_sensitive_text(base_raw);
        return ClassifiedProviderFailure {
            class: ProviderFailureClass::ProviderError,
            code: code_norm.or_else(|| Some("acp_-32000".to_owned())),
            message: format!("ACP agent request rejected (-32000): {detail}"),
            retryable: false,
            retry_budget_ms: None,
        };
    }

    ClassifiedProviderFailure {
        class: ProviderFailureClass::Unknown,
        code: code_norm,
        message: redact_sensitive_text(base_raw),
        retryable: false,
        retry_budget_ms: None,
    }
}

fn classify_specific_failure(
    input: &ProviderFailureInput,
    base_raw: &str,
    lower: &str,
    code_norm: Option<String>,
) -> Option<ClassifiedProviderFailure> {
    let is_http_413 = input.http_status == Some(413)
        || lower.contains("payload too large")
        || lower.contains("request entity too large")
        || lower.contains("http 413");
    if is_http_413 {
        let detail = redact_sensitive_text(base_raw);
        return Some(ClassifiedProviderFailure {
            class: ProviderFailureClass::ValidationError,
            code: code_norm.or_else(|| Some("http_413".to_owned())),
            message: format!("Request payload too large (HTTP 413, non-retryable): {detail}"),
            retryable: false,
            retry_budget_ms: None,
        });
    }

    if input.is_macos
        && (input.os_error_code.as_deref() == Some("EPERM")
            || lower.contains("eperm")
            || lower.contains("operation not permitted"))
    {
        let detail = redact_sensitive_text(base_raw);
        return Some(ClassifiedProviderFailure {
            class: ProviderFailureClass::PermissionError,
            code: code_norm.or_else(|| Some("EPERM".to_owned())),
            message: format!(
                "{detail} (macOS TCC may be blocking folder access; grant Full Disk Access or Desktop/Documents access in System Settings)"
            ),
            retryable: false,
            retry_budget_ms: None,
        });
    }

    if matches!(input.http_status, Some(401 | 403))
        || lower.contains("unauthorized")
        || lower.contains("authentication failed")
        || lower.contains("invalid api key")
        || lower.contains("needs auth")
    {
        return Some(ClassifiedProviderFailure {
            class: ProviderFailureClass::PermissionError,
            code: code_norm.or_else(|| input.http_status.map(|s| format!("http_{s}"))),
            message: redact_sensitive_text(base_raw),
            retryable: false,
            retry_budget_ms: None,
        });
    }

    classify_limit_or_transport(input, base_raw, lower, code_norm)
}

fn classify_limit_or_transport(
    input: &ProviderFailureInput,
    base_raw: &str,
    lower: &str,
    code_norm: Option<String>,
) -> Option<ClassifiedProviderFailure> {
    if input.http_status == Some(429)
        || lower.contains("rate limit")
        || lower.contains("too many requests")
        || lower.contains("quota exceeded")
    {
        return Some(ClassifiedProviderFailure {
            class: ProviderFailureClass::UsageLimit,
            code: code_norm.or_else(|| Some("rate_limited".to_owned())),
            message: redact_sensitive_text(base_raw),
            retryable: true,
            retry_budget_ms: Some(PROVIDER_RETRY_BUDGET_MS),
        });
    }

    if input.os_error_code.as_deref() == Some("ENOENT")
        || lower.contains("enoent")
        || lower.contains("not installed")
        || lower.contains("command not found")
    {
        return Some(ClassifiedProviderFailure {
            class: ProviderFailureClass::ProviderError,
            code: code_norm.or_else(|| Some("ENOENT".to_owned())),
            message: redact_sensitive_text(base_raw),
            retryable: false,
            retry_budget_ms: None,
        });
    }

    if lower.contains("context length")
        || lower.contains("maximum context")
        || lower.contains("prompt is too long")
    {
        return Some(ClassifiedProviderFailure {
            class: ProviderFailureClass::ValidationError,
            code: code_norm,
            message: redact_sensitive_text(base_raw),
            retryable: false,
            retry_budget_ms: None,
        });
    }

    if matches!(input.http_status, Some(502..=504))
        || lower.contains("econnreset")
        || lower.contains("etimedout")
        || lower.contains("timed out")
    {
        return Some(ClassifiedProviderFailure {
            class: ProviderFailureClass::TransportError,
            code: code_norm,
            message: redact_sensitive_text(base_raw),
            retryable: true,
            retry_budget_ms: Some(PROVIDER_RETRY_BUDGET_MS),
        });
    }

    None
}
