//! Async `reqwest` HTTP execution engine with strict same-origin redirects and rate limiting.

use std::time::{SystemTime, UNIX_EPOCH};

use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use serde_json::Value;

use crate::checkpoints::CheckpointError;
use crate::core::CoreError;
use crate::core::rate_limit::SourceControlRateLimit;
use crate::forges::{ForgeError, HttpRequestSpec, is_same_origin};

/// Maximum HTTP redirects followed automatically within the same trusted origin.
pub const MAX_REDIRECTS: usize = 5;

/// Default maximum `GitHub` GraphQL response payload bytes decoded (`8 MiB`, `#39928`).
pub const DEFAULT_MAX_GRAPHQL_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

/// Errors raised by live HTTP, git/forge CLI, and forge client operations.
#[derive(Debug, thiserror::Error)]
pub enum LiveClientError {
    /// Forge-level host validation, credential, redirect, or payload error.
    #[error(transparent)]
    Forge(#[from] ForgeError),
    /// Rate-limit admission or remote discovery error.
    #[error(transparent)]
    Core(#[from] CoreError),
    /// Checkpoint diff computation error.
    #[error(transparent)]
    Checkpoint(#[from] CheckpointError),
    /// Transport-level `reqwest` failure or malformed header.
    #[error("HTTP transport error: {0}")]
    Http(String),
    /// Non-success HTTP response status returned by upstream.
    #[error("HTTP {status} from `{url}`: {body}")]
    HttpStatus {
        /// HTTP status code returned by upstream.
        status: u16,
        /// Final destination URL.
        url: String,
        /// Truncated/raw response body.
        body: String,
    },
    /// Failed to spawn or wait on a CLI subprocess.
    #[error("CLI process error for `{program}`: {reason}")]
    Cli {
        /// Invoked executable name or path.
        program: String,
        /// Underlying failure explanation.
        reason: String,
    },
    /// Invoked CLI subprocess exited with a non-zero exit status.
    #[error("CLI `{program}` exited with {code:?}: {stderr}")]
    CliExit {
        /// Invoked executable name or path.
        program: String,
        /// Process exit code when available.
        code: Option<i32>,
        /// Captured `stderr` output.
        stderr: String,
    },
    /// Caller-supplied invalid argument or missing file.
    #[error("invalid live client input: {0}")]
    InvalidInput(String),
}

/// Completed HTTP response captured after redirect verification and rate-limit accounting.
#[derive(Debug, Clone)]
pub struct LiveHttpResponse {
    /// Final HTTP response status code.
    pub status: StatusCode,
    /// Upstream response headers.
    pub headers: HeaderMap,
    /// Full response body text (`UTF-8` lossy).
    pub body: String,
    /// Final URL reached after same-origin redirects.
    pub final_url: String,
}

impl LiveHttpResponse {
    /// Parses [`Self::body`] as `serde_json::Value`.
    pub fn json(&self) -> Result<Value, LiveClientError> {
        serde_json::from_str(&self.body).map_err(|err| {
            ForgeError::MalformedResponse(format!("invalid JSON body: {err}")).into()
        })
    }
}

/// Constructs a hardened `reqwest::Client` with automatic redirects disabled.
pub fn build_http_client() -> Result<reqwest::Client, LiveClientError> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("legioncode")
        .build()
        .map_err(|err| LiveClientError::Http(format!("failed to build HTTP client: {err}")))
}

/// Executes an [`HttpRequestSpec`] through `client`, enforcing `rate_limit` per host
/// and validating any 3xx `Location` stays strictly same-origin before following.
pub async fn execute_http_spec(
    client: &reqwest::Client,
    rate_limit: &SourceControlRateLimit,
    spec: &HttpRequestSpec,
    allow_reserve: bool,
) -> Result<LiveHttpResponse, LiveClientError> {
    execute_http_spec_with_max_bytes(client, rate_limit, spec, allow_reserve, None).await
}

/// Streams chunked HTTP response bytes up to `limit`, aborting and discarding before UTF-8/JSON
/// decoding when the byte cap would be exceeded (`#39928`).
pub async fn collect_response_text_within_limit(
    mut resp: reqwest::Response,
    url: &str,
    limit: Option<usize>,
) -> Result<String, LiveClientError> {
    let Some(max_bytes) = limit else {
        return resp.text().await.map_err(|err| {
            LiveClientError::Http(format!("reading body from `{url}` failed: {err}"))
        });
    };

    let mut buf = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|err| {
        LiveClientError::Http(format!("streaming body from `{url}` failed: {err}"))
    })? {
        if buf.len().saturating_add(chunk.len()) > max_bytes {
            return Err(LiveClientError::Forge(ForgeError::MalformedResponse(
                format!("HTTP response from `{url}` exceeded {max_bytes}-byte decode limit"),
            )));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Executes an [`HttpRequestSpec`] while optionally capping streamed response bytes before decoding (`#39928`).
pub async fn execute_http_spec_with_max_bytes(
    client: &reqwest::Client,
    rate_limit: &SourceControlRateLimit,
    spec: &HttpRequestSpec,
    allow_reserve: bool,
    max_response_bytes: Option<usize>,
) -> Result<LiveHttpResponse, LiveClientError> {
    let mut current_method = spec.method.clone();
    let mut current_url = spec.url.clone();
    let mut current_body = spec.body.clone();

    for _hop in 0..=MAX_REDIRECTS {
        let host = extract_url_host(&current_url)?;
        let now_secs = current_epoch_secs();
        let permit = rate_limit.acquire(&host, allow_reserve, now_secs)?;

        let mut req = client.request(current_method.clone(), &current_url);
        for (name, val) in &spec.headers {
            let hdr_name = HeaderName::from_bytes(name.as_bytes()).map_err(|err| {
                LiveClientError::Http(format!("invalid header name `{name}`: {err}"))
            })?;
            let hdr_val = HeaderValue::from_str(val).map_err(|err| {
                LiveClientError::Http(format!("invalid header value for `{name}`: {err}"))
            })?;
            req = req.header(hdr_name, hdr_val);
        }
        if let Some(body) = &current_body {
            req = req.body(body.clone());
        }

        let resp = req.send().await.map_err(|err| {
            LiveClientError::Http(format!("request to `{current_url}` failed: {err}"))
        })?;

        let status = resp.status();
        let headers = resp.headers().clone();
        let body_text =
            collect_response_text_within_limit(resp, &current_url, max_response_bytes).await?;

        let after_secs = current_epoch_secs();
        let _ = rate_limit.update_from_headers(&host, status, &headers, after_secs)?;
        drop(permit);

        if status.is_redirection()
            && let Some(loc_val) = headers.get(http::header::LOCATION)
        {
            let loc_str = loc_val.to_str().map_err(|_| {
                LiveClientError::Forge(ForgeError::UntrustedUrl(
                    "non-UTF-8 Location header".to_owned(),
                ))
            })?;
            let next_url = resolve_same_origin_redirect(&current_url, loc_str)?;
            if matches!(
                status,
                StatusCode::MOVED_PERMANENTLY | StatusCode::FOUND | StatusCode::SEE_OTHER
            ) && current_method != Method::GET
                && current_method != Method::HEAD
            {
                current_method = Method::GET;
                current_body = None;
            }
            current_url = next_url;
            continue;
        }

        return Ok(LiveHttpResponse {
            status,
            headers,
            body: body_text,
            final_url: current_url,
        });
    }

    Err(LiveClientError::Forge(ForgeError::UntrustedUrl(format!(
        "exceeded maximum redirect hops ({MAX_REDIRECTS}) at `{current_url}`"
    ))))
}

/// Validates HTTP(S) URL syntax and extracts normalized `host[:port]` key for rate limiting.
pub fn extract_url_host(url: &str) -> Result<String, LiveClientError> {
    let trimmed = url.trim();
    let (scheme, rest) = trimmed
        .split_once("://")
        .ok_or_else(|| LiveClientError::InvalidInput(format!("URL missing scheme: `{trimmed}`")))?;
    let scheme_lower = scheme.to_ascii_lowercase();
    if scheme_lower != "https" && scheme_lower != "http" {
        return Err(LiveClientError::Forge(ForgeError::UntrustedUrl(
            trimmed.to_owned(),
        )));
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') {
        return Err(LiveClientError::Forge(ForgeError::UntrustedUrl(
            trimmed.to_owned(),
        )));
    }
    let host_only = strip_port_from_authority(authority);
    if host_only.is_empty() {
        return Err(LiveClientError::Forge(ForgeError::UntrustedUrl(
            trimmed.to_owned(),
        )));
    }
    Ok(authority.to_ascii_lowercase())
}

/// Ensures redirect targets stay strictly within `current_url`'s scheme and `host[:port]`.
pub fn resolve_same_origin_redirect(
    current_url: &str,
    location: &str,
) -> Result<String, LiveClientError> {
    let trimmed = location.trim();
    if trimmed.is_empty() || trimmed.starts_with("//") {
        return Err(LiveClientError::Forge(ForgeError::UntrustedUrl(
            trimmed.to_owned(),
        )));
    }
    if trimmed.contains("://") {
        if !is_same_origin(current_url, trimmed) {
            return Err(LiveClientError::Forge(ForgeError::UntrustedUrl(
                trimmed.to_owned(),
            )));
        }
        return Ok(trimmed.to_owned());
    }
    let origin = extract_origin_prefix(current_url)?;
    if trimmed.starts_with('/') {
        Ok(format!("{origin}{trimmed}"))
    } else {
        Ok(format!("{origin}/{trimmed}"))
    }
}

fn extract_origin_prefix(url: &str) -> Result<String, LiveClientError> {
    let (scheme, rest) = url
        .trim()
        .split_once("://")
        .ok_or_else(|| LiveClientError::Forge(ForgeError::UntrustedUrl(url.to_owned())))?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() {
        return Err(LiveClientError::Forge(ForgeError::UntrustedUrl(
            url.to_owned(),
        )));
    }
    Ok(format!("{}://{authority}", scheme.to_ascii_lowercase()))
}

fn strip_port_from_authority(authority: &str) -> &str {
    if let Some(bracketed) = authority.strip_prefix('[')
        && let Some((host, _)) = bracketed.split_once(']')
    {
        return host;
    }
    authority.rsplit_once(':').map_or(authority, |(h, _)| h)
}

/// Returns current Unix epoch seconds cleanly without panicking.
#[must_use]
pub fn current_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
