//! HTTP method, `Origin`, `Content-Type`, and `Accept` gating for `/mcp`.
//!
//! Mirrors `.repos/effect/packages/effect/src/ai/McpServer.ts` and
//! `apps/server/src/mcp/McpHttpServer.ts`.

use thiserror::Error;

/// Incoming HTTP metadata evaluated at the `/mcp` pre-auth and media-type gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McpHttpRequest<'a> {
    /// HTTP request method (for example `"POST"`, `"GET"`, `"DELETE"`).
    pub method: &'a str,
    /// Raw HTTP `Origin` header value, if supplied.
    pub origin: Option<&'a str>,
    /// Raw HTTP `Content-Type` header value, if supplied.
    pub content_type: Option<&'a str>,
    /// Raw HTTP `Accept` header value, if supplied.
    pub accept: Option<&'a str>,
    /// Whether `DELETE` session termination requests are permitted.
    pub allow_session_termination: bool,
}

impl<'a> McpHttpRequest<'a> {
    /// Builds a standard `/mcp` request descriptor with `DELETE` termination enabled.
    pub fn new(
        method: &'a str,
        origin: Option<&'a str>,
        content_type: Option<&'a str>,
        accept: Option<&'a str>,
    ) -> Self {
        Self {
            method,
            origin,
            content_type,
            accept,
            allow_session_termination: true,
        }
    }
}

/// Permitted `/mcp` request dispatch action after passing HTTP protocol gates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpGateOutcome {
    /// Handle an authenticated JSON-RPC `POST` request.
    JsonRpcPost,
    /// Terminate an existing MCP session via `DELETE`.
    TerminateSession,
}

/// HTTP rejection produced by [`evaluate_mcp_http_request`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("MCP HTTP {status} ({code}): {message}")]
pub struct McpGateRejection {
    /// HTTP status code (`403`, `405`, `406`, or `415`).
    pub status: u16,
    /// Optional `Allow` response header for `405 Method Not Allowed`.
    pub allow: Option<&'static str>,
    /// Machine-readable protocol rejection identifier.
    pub code: &'static str,
    /// Human-readable error explanation.
    pub message: String,
}

/// Parses comma-separated MIME media types from `Accept` or `Content-Type` headers.
///
/// Strips `;parameter=value` suffixes, skips `q=0` entries, and normalizes
/// media type tokens to ASCII lowercase.
pub fn parse_mcp_media_types(header_value: Option<&str>) -> Vec<String> {
    let Some(raw) = header_value else {
        return Vec::new();
    };
    let mut types = Vec::new();
    for segment in raw.split(',') {
        let mut parts = segment.split(';');
        let Some(media_raw) = parts.next() else {
            continue;
        };
        let media = media_raw.trim().to_ascii_lowercase();
        if media.is_empty() {
            continue;
        }
        let mut zero_q = false;
        for param in parts {
            let trimmed = param.trim();
            if let Some((key, val)) = trimmed.split_once('=')
                && key.trim().eq_ignore_ascii_case("q")
                && let Ok(q_val) = val.trim().parse::<f64>()
                && q_val <= 0.0
            {
                zero_q = true;
            }
        }
        if !zero_q {
            types.push(media);
        }
    }
    types
}

/// Evaluates `/mcp` HTTP boundary checks in strict pre-auth protocol order.
///
/// Order enforced:
/// 1. Any `Origin` header (even empty) is rejected with `403` before auth/method checks.
/// 2. Only `POST` and optionally `DELETE` are permitted; others fail with `405` + `Allow`.
/// 3. `POST` requires primary `Content-Type` `application/json` (`415` otherwise).
/// 4. `POST` requires `Accept` to include both `application/json` and `text/event-stream` (`406`).
pub fn evaluate_mcp_http_request(
    request: &McpHttpRequest<'_>,
) -> Result<McpGateOutcome, McpGateRejection> {
    if request.origin.is_some() {
        return Err(McpGateRejection {
            status: 403,
            allow: None,
            code: "forbidden_origin",
            message: "Browser Origin headers are not permitted on /mcp".to_owned(),
        });
    }

    let method = request.method.trim().to_ascii_uppercase();
    let allow_header = if request.allow_session_termination {
        "POST, DELETE"
    } else {
        "POST"
    };

    match method.as_str() {
        "DELETE" => {
            if request.allow_session_termination {
                Ok(McpGateOutcome::TerminateSession)
            } else {
                Err(McpGateRejection {
                    status: 405,
                    allow: Some("POST"),
                    code: "method_not_allowed",
                    message: "Method DELETE is not allowed on /mcp".to_owned(),
                })
            }
        }
        "POST" => validate_post_media_headers(request),
        other => Err(McpGateRejection {
            status: 405,
            allow: Some(allow_header),
            code: "method_not_allowed",
            message: format!("Method {other} is not allowed on /mcp"),
        }),
    }
}

fn validate_post_media_headers(
    request: &McpHttpRequest<'_>,
) -> Result<McpGateOutcome, McpGateRejection> {
    let content_types = parse_mcp_media_types(request.content_type);
    if content_types.first().map(String::as_str) != Some("application/json") {
        return Err(McpGateRejection {
            status: 415,
            allow: None,
            code: "unsupported_media_type",
            message: "Content-Type must be application/json".to_owned(),
        });
    }

    let accept_types = parse_mcp_media_types(request.accept);
    let has_json = accept_types.iter().any(|item| item == "application/json");
    let has_sse = accept_types.iter().any(|item| item == "text/event-stream");
    if !has_json || !has_sse {
        return Err(McpGateRejection {
            status: 406,
            allow: None,
            code: "not_acceptable",
            message: "Accept header must include both application/json and text/event-stream"
                .to_owned(),
        });
    }

    Ok(McpGateOutcome::JsonRpcPost)
}
