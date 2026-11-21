//! Request credential precedence (`Authorization` before cookies) and `DPoP` scheme binding.
//!
//! Fixes BUG-03 in `apps/server/src/auth/EnvironmentAuth.ts`: an `Authorization`
//! header (`Bearer` or `DPoP`) always outranks ambient cookies, and presenting a
//! malformed, empty, or unsupported `Authorization` header never silently falls
//! back to cookies.

use t3_shared::dpop::{
    DpopVerificationError, DpopVerifiedClaims, VerifyDpopProofInput, verify_dpop_proof,
};
use thiserror::Error;

/// Transport source that yielded a request credential.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CredentialSource {
    /// `Authorization: Bearer <token>` header.
    AuthorizationBearer,
    /// `Authorization: DPoP <token>` header.
    AuthorizationDpop,
    /// Primary environment session cookie.
    PrimaryCookie,
    /// Legacy environment session cookie.
    LegacyCookie,
    /// Reusable local dev cookie fallback.
    DevCookie,
}

impl CredentialSource {
    /// Returns `true` when the credential came from an `Authorization` header.
    pub fn is_authorization_header(self) -> bool {
        matches!(self, Self::AuthorizationBearer | Self::AuthorizationDpop)
    }

    /// Returns `true` when the credential came from any session cookie.
    pub fn is_cookie(self) -> bool {
        !self.is_authorization_header()
    }
}

/// Extracted credential token paired with its winning source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedCredential {
    /// Trimmed credential token string.
    pub token: String,
    /// Source slot selected under [`CredentialPrecedence`].
    pub source: CredentialSource,
}

/// Candidate credential slots extracted from an HTTP or WebSocket upgrade request.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RequestCredentials<'a> {
    /// Raw value of the `Authorization` header, if present.
    pub authorization: Option<&'a str>,
    /// Value of the primary cookie, if present.
    pub primary_cookie: Option<&'a str>,
    /// Value of the legacy fallback cookie, if present.
    pub legacy_cookie: Option<&'a str>,
    /// Value of the dev credential cookie, if present.
    pub dev_cookie: Option<&'a str>,
}

/// Failure raised when selecting a request credential under [`CredentialPrecedence`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CredentialSelectionError {
    /// An `Authorization` header was present but blank.
    #[error("Authorization header is present but empty")]
    EmptyAuthorizationHeader,
    /// An `Authorization` `Bearer` or `DPoP` scheme was present without a token.
    #[error("Authorization {scheme} header is missing its token")]
    EmptyAuthorizationToken {
        /// Canonical scheme name (`Bearer` or `DPoP`).
        scheme: &'static str,
    },
    /// Token portion after `Bearer` or `DPoP` contained whitespace.
    #[error("Authorization {scheme} token contains invalid whitespace")]
    MalformedAuthorizationToken {
        /// Canonical scheme name (`Bearer` or `DPoP`).
        scheme: &'static str,
    },
    /// An `Authorization` header used a scheme other than `Bearer` or `DPoP`.
    #[error("unsupported Authorization scheme {scheme:?}")]
    UnsupportedAuthorizationScheme {
        /// Scheme token supplied by the caller.
        scheme: String,
    },
    /// Neither an `Authorization` header nor any non-empty cookie was present.
    #[error("no authentication credential present")]
    MissingCredential,
}

/// Failure raised when validating `DPoP` key binding against a selected credential.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DpopBindingError {
    /// Session is bound to a `DPoP` key thumbprint, so `Bearer`/cookies are rejected.
    #[error("DPoP-bound session requires Authorization: DPoP scheme")]
    DpopSchemeRequired,
    /// Request presented `Authorization: DPoP` for a session without key binding.
    #[error("unexpected DPoP scheme for non-DPoP session")]
    UnexpectedDpopForUnboundSession,
    /// Cryptographic or claim verification of the `DPoP` proof failed.
    #[error("invalid DPoP proof: {0}")]
    Verification(#[from] DpopVerificationError),
}

/// Precedence resolver enforcing `Authorization` > primary > legacy > dev cookies.
#[derive(Debug, Clone, Copy, Default)]
pub struct CredentialPrecedence;

impl CredentialPrecedence {
    /// Selects the highest-precedence credential from `input`.
    ///
    /// Whenever `input.authorization` is `Some`, cookies are never consulted even
    /// if parsing fails.
    pub fn select(
        input: &RequestCredentials<'_>,
    ) -> Result<SelectedCredential, CredentialSelectionError> {
        if let Some(raw_auth) = input.authorization {
            return parse_authorization_header(raw_auth);
        }

        let cookie_slots = [
            (input.primary_cookie, CredentialSource::PrimaryCookie),
            (input.legacy_cookie, CredentialSource::LegacyCookie),
            (input.dev_cookie, CredentialSource::DevCookie),
        ];

        for (candidate, source) in cookie_slots {
            if let Some(trimmed) = candidate.map(str::trim).filter(|val| !val.is_empty()) {
                return Ok(SelectedCredential {
                    token: trimmed.to_owned(),
                    source,
                });
            }
        }

        Err(CredentialSelectionError::MissingCredential)
    }
}

fn parse_authorization_header(raw: &str) -> Result<SelectedCredential, CredentialSelectionError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(CredentialSelectionError::EmptyAuthorizationHeader);
    }

    let (scheme, rest) = match trimmed.find(char::is_whitespace) {
        Some(idx) => (&trimmed[..idx], trimmed[idx..].trim()),
        None => (trimmed, ""),
    };

    let (canonical, source) = match scheme.to_ascii_lowercase().as_str() {
        "bearer" => ("Bearer", CredentialSource::AuthorizationBearer),
        "dpop" => ("DPoP", CredentialSource::AuthorizationDpop),
        _ => {
            return Err(CredentialSelectionError::UnsupportedAuthorizationScheme {
                scheme: scheme.to_owned(),
            });
        }
    };

    if rest.is_empty() {
        return Err(CredentialSelectionError::EmptyAuthorizationToken { scheme: canonical });
    }
    if rest.contains(char::is_whitespace) {
        return Err(CredentialSelectionError::MalformedAuthorizationToken { scheme: canonical });
    }

    Ok(SelectedCredential {
        token: rest.to_owned(),
        source,
    })
}

/// Extracts a trimmed non-empty cookie value by name from an HTTP `Cookie` header.
pub fn extract_cookie_value(cookie_header: &str, cookie_name: &str) -> Option<String> {
    if cookie_name.trim().is_empty() {
        return None;
    }

    for part in cookie_header.split(';') {
        let trimmed = part.trim();
        let Some((name, value)) = trimmed.split_once('=') else {
            continue;
        };
        if name.trim() == cookie_name {
            let clean_val = value.trim();
            if !clean_val.is_empty() {
                return Some(clean_val.to_owned());
            }
        }
    }

    None
}

/// Validates that `credential` satisfies the session's `DPoP` thumbprint policy.
pub fn verify_dpop_credential_binding(
    credential: &SelectedCredential,
    session_proof_thumbprint: Option<&str>,
    dpop_proof: Option<&str>,
    method: &str,
    url: &str,
    now_epoch_seconds: i64,
    max_age_seconds: Option<i64>,
) -> Result<Option<DpopVerifiedClaims>, DpopBindingError> {
    if let Some(expected_thumbprint) = session_proof_thumbprint {
        if credential.source != CredentialSource::AuthorizationDpop {
            return Err(DpopBindingError::DpopSchemeRequired);
        }
        let claims = verify_dpop_proof(&VerifyDpopProofInput {
            proof: dpop_proof,
            method,
            url,
            now_epoch_seconds,
            expected_thumbprint: Some(expected_thumbprint),
            expected_access_token: Some(&credential.token),
            max_age_seconds,
        })?;
        Ok(Some(claims))
    } else {
        if credential.source == CredentialSource::AuthorizationDpop {
            return Err(DpopBindingError::UnexpectedDpopForUnboundSession);
        }
        Ok(None)
    }
}
