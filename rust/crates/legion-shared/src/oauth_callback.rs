//! OAuth loopback callback URL validation and code/state extraction (`oauthCallback.ts`).

use thiserror::Error;

use crate::url_authority::{ParsedUrlParts, is_valid_scheme, parse_url_parts};
use crate::utf16::utf8_len_to_utf16;

/// Maximum accepted callback URL length matching JavaScript `value.length > 16_384`.
pub const MAX_OAUTH_CALLBACK_LEN_UTF16: usize = 16_384;

/// Validation errors returned when a pasted OAuth redirect URL fails checks.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum OAuthCallbackError {
    /// Input URL or redirect URI could not be parsed.
    #[error("Invalid OAuth callback URL.")]
    InvalidUrl,
    /// Input violates sign-in binding (`origin`, `pathname`, `state`, `code`/`error`, or `client_id`).
    #[error("This redirect URL does not belong to the current sign-in.")]
    RedirectMismatch,
}

/// Validated OAuth callback URL and extracted query parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedOAuthCallback {
    /// Normalized scheme + host + optional non-default port.
    pub origin: String,
    /// Callback path starting with `/`.
    pub pathname: String,
    /// OAuth `state` parameter matching the active sign-in flow.
    pub state: String,
    /// Authorization code when callback returned `?code=...`.
    pub code: Option<String>,
    /// OAuth error identifier when provider returned `?error=...`.
    pub error: Option<String>,
    /// Optional `error_description` parameter when present.
    pub error_description: Option<String>,
    /// Optional `client_id` parameter when present.
    pub client_id: Option<String>,
    /// All decoded query parameters in URL order.
    pub query_params: Vec<(String, String)>,
}

/// Parses a URL pasted from a browser address bar, which may omit `scheme://` (`oauthCallback.ts:43`).
pub fn pasted_url(
    value: &str,
    default_protocol: Option<&str>,
) -> Result<ParsedOAuthUrl, OAuthCallbackError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(OAuthCallbackError::InvalidUrl);
    }
    let has_scheme = trimmed
        .find("://")
        .is_some_and(|idx| is_valid_scheme(&trimmed[..idx]));
    let candidate = if has_scheme {
        trimmed.to_owned()
    } else {
        let proto = default_protocol
            .unwrap_or("http")
            .trim()
            .trim_end_matches(':');
        if !is_valid_scheme(proto) {
            return Err(OAuthCallbackError::InvalidUrl);
        }
        format!("{proto}://{trimmed}")
    };
    let parts = parse_url_parts(&candidate).ok_or(OAuthCallbackError::InvalidUrl)?;
    Ok(ParsedOAuthUrl { parts })
}

/// Opaque parsed URL wrapper returned by [`pasted_url`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedOAuthUrl {
    parts: ParsedUrlParts,
}

impl ParsedOAuthUrl {
    /// Normalized origin (`scheme://host[:port]`).
    #[must_use]
    pub fn origin(&self) -> &str {
        &self.parts.origin
    }

    /// Path starting with `/`.
    #[must_use]
    pub fn pathname(&self) -> &str {
        &self.parts.pathname
    }

    /// Returns all query values matching `name`.
    #[must_use]
    pub fn query_all(&self, name: &str) -> Vec<&str> {
        self.parts
            .query_params
            .iter()
            .filter_map(|(k, v)| (k == name).then_some(v.as_str()))
            .collect()
    }
}

/// Validates a pasted browser redirect URL against the expected `redirect_uri` and `state` (`oauthCallback.ts:6`).
pub fn oauth_callback_url(
    value: &str,
    redirect_uri: &str,
    expected_state: &str,
) -> Result<ValidatedOAuthCallback, OAuthCallbackError> {
    let expected = parse_url_parts(redirect_uri).ok_or(OAuthCallbackError::InvalidUrl)?;
    let callback = pasted_url(value, Some(&expected.scheme))?;
    let parts = &callback.parts;

    let states = callback.query_all("state");
    let codes = callback.query_all("code");
    let errors = callback.query_all("error");

    let has_valid_outcome = (codes.len() == 1 && !codes[0].is_empty() && errors.is_empty())
        || (errors.len() == 1 && !errors[0].is_empty() && codes.is_empty());

    let fragment_non_empty = parts.fragment.as_deref().is_some_and(|f| !f.is_empty());

    if utf8_len_to_utf16(value) > MAX_OAUTH_CALLBACK_LEN_UTF16
        || parts.origin != expected.origin
        || parts.pathname != expected.pathname
        || parts.has_userinfo
        || fragment_non_empty
        || states.len() != 1
        || states[0] != expected_state
        || !has_valid_outcome
    {
        return Err(OAuthCallbackError::RedirectMismatch);
    }

    let error_description = callback
        .query_all("error_description")
        .first()
        .map(|s| (*s).to_owned());
    let client_id = callback
        .query_all("client_id")
        .first()
        .map(|s| (*s).to_owned());

    Ok(ValidatedOAuthCallback {
        origin: parts.origin.clone(),
        pathname: parts.pathname.clone(),
        state: states[0].to_owned(),
        code: codes.first().map(|s| (*s).to_owned()),
        error: errors.first().map(|s| (*s).to_owned()),
        error_description,
        client_id,
        query_params: parts.query_params.clone(),
    })
}

/// Validates a Codex OAuth callback URL, enforcing single valid `oaiapp_*` client ID when present (`codexAuthHandoff.ts:59`).
pub fn validate_codex_callback_url(
    value: &str,
    redirect_uri: &str,
    expected_state: &str,
) -> Result<ValidatedOAuthCallback, OAuthCallbackError> {
    let parsed = oauth_callback_url(value, redirect_uri, expected_state)?;
    let clients: Vec<&str> = parsed
        .query_params
        .iter()
        .filter_map(|(k, v)| (k == "client_id").then_some(v.as_str()))
        .collect();
    let valid_client = clients.is_empty()
        || (clients.len() == 1
            && clients[0].strip_prefix("oaiapp_").is_some_and(|rest| {
                !rest.is_empty()
                    && rest
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            }));
    if !valid_client {
        return Err(OAuthCallbackError::RedirectMismatch);
    }
    Ok(parsed)
}
