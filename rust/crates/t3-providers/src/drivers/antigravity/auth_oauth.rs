//! Antigravity 1.5 CLI stored OAuth token evaluation and Go-keyring secret decoding.

use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::auth::AntigravityAuthMethod;

const GO_KEYRING_BASE64_PREFIX: &str = "go-keyring-base64:";
const GO_KEYRING_HEX_PREFIX: &str = "go-keyring-encoded:";
const CLI_TOKEN_EXPIRY_LEEWAY_MS: i64 = 60_000;
const OAUTH_EXPIRED_MESSAGE: &str = "Antigravity's saved sign-in has expired. Start an Antigravity turn to renew it, then refresh limits.";
const OAUTH_MISSING_MESSAGE: &str = "Sign in with Google to see Antigravity limits.";

/// Errors returned while decoding Go keyring secrets stored by `agy`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AntigravitySecretDecodeError {
    /// Base64 payload contained invalid bytes or padding.
    #[error("invalid go-keyring base64 secret payload")]
    InvalidBase64,
    /// Hex payload contained invalid digits.
    #[error("invalid go-keyring hex secret payload: {0}")]
    InvalidHex(String),
    /// Decoded bytes were not valid UTF-8.
    #[error("decoded go-keyring secret is not valid UTF-8")]
    InvalidUtf8,
}

/// Decodes a `go-keyring-base64:` or `go-keyring-encoded:` secret into UTF-8 (`5283f33ca9`).
pub fn decode_go_keyring_secret(bytes: &[u8]) -> Result<String, AntigravitySecretDecodeError> {
    let raw = std::str::from_utf8(bytes).map_err(|_| AntigravitySecretDecodeError::InvalidUtf8)?;
    if let Some(encoded) = raw.strip_prefix(GO_KEYRING_BASE64_PREFIX) {
        let decoded = decode_base64_str(encoded)?;
        return String::from_utf8(decoded).map_err(|_| AntigravitySecretDecodeError::InvalidUtf8);
    }
    if let Some(encoded) = raw.strip_prefix(GO_KEYRING_HEX_PREFIX) {
        let decoded = hex::decode(encoded.trim())
            .map_err(|err| AntigravitySecretDecodeError::InvalidHex(err.to_string()))?;
        return String::from_utf8(decoded).map_err(|_| AntigravitySecretDecodeError::InvalidUtf8);
    }
    Ok(raw.to_owned())
}

fn decode_base64_str(input: &str) -> Result<Vec<u8>, AntigravitySecretDecodeError> {
    let clean: Vec<u8> = input.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if !clean.len().is_multiple_of(4) {
        return Err(AntigravitySecretDecodeError::InvalidBase64);
    }
    let mut out = Vec::with_capacity((clean.len() / 4) * 3);
    let (chunks, _) = clean.as_chunks::<4>();
    for chunk in chunks {
        let a = b64_val(chunk[0]).ok_or(AntigravitySecretDecodeError::InvalidBase64)?;
        let b = b64_val(chunk[1]).ok_or(AntigravitySecretDecodeError::InvalidBase64)?;
        let pad_c = chunk[2] == b'=';
        let pad_d = chunk[3] == b'=';
        if pad_c && !pad_d {
            return Err(AntigravitySecretDecodeError::InvalidBase64);
        }
        let c = if pad_c {
            0
        } else {
            b64_val(chunk[2]).ok_or(AntigravitySecretDecodeError::InvalidBase64)?
        };
        let d = if pad_d {
            0
        } else {
            b64_val(chunk[3]).ok_or(AntigravitySecretDecodeError::InvalidBase64)?
        };
        out.push((a << 2) | (b >> 4));
        if !pad_c {
            out.push((b << 4) | (c >> 2));
        }
        if !pad_d {
            out.push((c << 6) | d);
        }
    }
    Ok(out)
}

const fn b64_val(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' | b'-' => Some(62),
        b'/' | b'_' => Some(63),
        _ => None,
    }
}

#[derive(Debug, Deserialize)]
struct RawCliTokenFile {
    token: AntigravityCliToken,
}

/// Saved `agy` CLI 1.5 OAuth token payload (`5283f33ca9`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AntigravityCliToken {
    /// OAuth bearer token used for quota / userinfo requests.
    pub access_token: String,
    /// Optional refresh token used to derive fallback account fingerprint.
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// Optional RFC-3339 expiry timestamp string.
    #[serde(default)]
    pub expiry: Option<String>,
}

/// Evaluation of an Antigravity 1.5 CLI stored sign-in (`5283f33ca9`, `084ad5e1a0`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AntigravityCliTokenStatus {
    /// Usable access token + deterministic fallback account SHA-256 fingerprint.
    Available {
        token: AntigravityCliToken,
        fallback_account_fingerprint: String,
    },
    /// No stored sign-in found for this instance/method.
    Missing { message: String },
    /// Stored sign-in exists, but has expired (or expires within 60 seconds).
    Expired { message: String },
    /// Stored payload could not be parsed.
    Invalid { message: String },
}

/// Parses JSON written into `antigravity-oauth-token` or the OS keyring (`5283f33ca9`).
#[must_use]
pub fn parse_antigravity_cli_token_json(raw: &str) -> Option<AntigravityCliToken> {
    let parsed: RawCliTokenFile = serde_json::from_str(raw).ok()?;
    if parsed.token.access_token.trim().is_empty() {
        return None;
    }
    Some(parsed.token)
}

/// Evaluates saved Antigravity 1.5 CLI OAuth state while isolating `oauth-personal` from
/// the shared OS keyring (`084ad5e1a0`).
#[must_use]
pub fn evaluate_antigravity_cli_token(
    profile_token_json: Option<&str>,
    shared_keyring_secret: Option<&[u8]>,
    auth_method: AntigravityAuthMethod,
    now_ms: i64,
) -> AntigravityCliTokenStatus {
    let owned_keyring;
    let stored = if let Some(file_json) = profile_token_json {
        file_json
    } else if auth_method.isolates_personal_oauth() {
        // oauth-personal must never fall back to the shared OS keyring (`084ad5e1a0`).
        return AntigravityCliTokenStatus::Missing {
            message: OAUTH_MISSING_MESSAGE.to_owned(),
        };
    } else if let Some(bytes) = shared_keyring_secret {
        match decode_go_keyring_secret(bytes) {
            Ok(decoded) => {
                owned_keyring = decoded;
                owned_keyring.as_str()
            }
            Err(err) => {
                return AntigravityCliTokenStatus::Invalid {
                    message: err.to_string(),
                };
            }
        }
    } else {
        return AntigravityCliTokenStatus::Missing {
            message: OAUTH_MISSING_MESSAGE.to_owned(),
        };
    };

    let Some(token) = parse_antigravity_cli_token_json(stored) else {
        return AntigravityCliTokenStatus::Invalid {
            message: "Antigravity's saved CLI sign-in payload is invalid.".to_owned(),
        };
    };

    let valid_until_ms = token
        .expiry
        .as_deref()
        .and_then(crate::core::parse_rfc3339_millis);
    if !matches!(valid_until_ms, Some(expiry_ms) if expiry_ms > now_ms + CLI_TOKEN_EXPIRY_LEEWAY_MS)
    {
        return AntigravityCliTokenStatus::Expired {
            message: OAUTH_EXPIRED_MESSAGE.to_owned(),
        };
    }

    let seed = token
        .refresh_token
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(&token.access_token);
    let fingerprint = hex::encode(Sha256::digest(
        format!("antigravity-cli\0{seed}").as_bytes(),
    ));
    AntigravityCliTokenStatus::Available {
        token,
        fallback_account_fingerprint: fingerprint,
    }
}
