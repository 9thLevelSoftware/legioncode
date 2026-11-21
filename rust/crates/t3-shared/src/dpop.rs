//! ES256 (P-256) `DPoP` proof signing, verification, and RFC 7638 JWK thumbprints.
//!
//! Mirrors `packages/shared/src/{dpop.ts,dpopCommon.ts}` and RFC 9449:
//! - JWT `typ` is `"dpop+jwt"` and `alg` is `"ES256"`.
//! - Embedded public `jwk` must be `EC` / `P-256` with no private `"d"` coordinate.
//! - `htu` is normalized by stripping query and fragment components.
//! - `iat` clock skew is bounded to [`DPOP_MAX_CLOCK_SKEW_SECONDS`] (`300` seconds) and checked
//!   only after validating key thumbprint, method, URL, optional `ath`, and signature.
//! - RFC 7638 thumbprints hash `{"crv":...,"kty":...,"x":...,"y":...}` in strict ASCII key order.

use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use p256::ecdsa::signature::Signer as _;
use p256::ecdsa::{Signature, SigningKey};
use p256::elliptic_curve::rand_core::OsRng;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

pub use crate::dpop_htu::normalize_dpop_htu;
use crate::dpop_htu::push_json_string;
use crate::dpop_verify::verify_dpop_proof_impl;

/// Expected JWT `typ` header value for `DPoP` proofs.
pub const DPOP_TYP: &str = "dpop+jwt";

/// Expected JWS `alg` header value for `DPoP` proofs.
pub const DPOP_ALG: &str = "ES256";

/// Default maximum tolerated clock skew (`|now - iat|`) in seconds (`300`s).
pub const DPOP_MAX_CLOCK_SKEW_SECONDS: i64 = 300;

/// Public EC P-256 JWK parameters carried inside a `DPoP` proof header.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DpopPublicJwk {
    /// Key type (always `"EC"` for P-256).
    pub kty: String,
    /// Elliptic curve name (always `"P-256"`).
    pub crv: String,
    /// Base64url-encoded 32-byte big-endian X coordinate.
    pub x: String,
    /// Base64url-encoded 32-byte big-endian Y coordinate.
    pub y: String,
}

/// Errors produced when loading a `DPoP` key or signing a `DPoP` proof.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DpopSignError {
    /// Provided raw scalar bytes do not form a valid P-256 private key.
    #[error("invalid P-256 private key bytes")]
    InvalidPrivateKey,
    /// HTTP method is empty.
    #[error("DPoP HTTP method must not be empty")]
    EmptyMethod,
    /// Target URL could not be normalized into a valid `htu`.
    #[error("invalid DPoP target URL")]
    InvalidUrl,
    /// Explicit `jti` was empty.
    #[error("DPoP jti must not be empty")]
    EmptyJti,
    /// JSON serialization failed unexpectedly.
    #[error("failed to serialize DPoP JWT component: {0}")]
    Serialization(String),
}

/// Structured failure codes returned by [`verify_dpop_proof`], matching `dpop.ts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DpopVerificationFailureCode {
    /// No `DPoP` proof header was supplied or it was blank.
    MissingProof,
    /// Compact JWT format, JSON, or required header/payload schema fields were invalid.
    MalformedProof,
    /// Embedded JWK thumbprint did not match `expected_thumbprint`.
    KeyMismatch,
    /// Payload `htm` did not match the request HTTP method.
    MethodMismatch,
    /// Payload `htu` did not match the normalized request URL.
    UrlMismatch,
    /// Payload `ath` did not match SHA-256(base64url) of `expected_access_token`.
    AccessTokenHashMismatch,
    /// Proof `iat` fell outside the allowed skew window.
    TimeWindow,
    /// ES256 signature verification failed.
    InvalidSignature,
    /// Base64url or EC key material could not be decoded.
    InvalidProof,
}

/// Verification failure returned by [`verify_dpop_proof`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{reason}")]
pub struct DpopVerificationError {
    /// Machine-readable failure classification matching `dpop.ts`.
    pub code: DpopVerificationFailureCode,
    /// Human-readable explanation matching `dpop.ts`.
    pub reason: String,
}

impl DpopVerificationError {
    pub(crate) fn new(code: DpopVerificationFailureCode, reason: impl Into<String>) -> Self {
        Self {
            code,
            reason: reason.into(),
        }
    }
}

/// ES256 P-256 signing key pair for generating `DPoP` proofs.
///
/// # Examples
///
/// ```
/// use t3_shared::dpop::{
///     DpopKeyPair, SignDpopProofInput, VerifyDpopProofInput, sign_dpop_proof, verify_dpop_proof,
/// };
///
/// let key_pair = DpopKeyPair::generate();
/// let proof = sign_dpop_proof(
///     &key_pair,
///     &SignDpopProofInput {
///         method: "POST",
///         url: "https://example.com/oauth/token?ignore=1#frag",
///         iat_epoch_seconds: 1_700_000_000,
///         jti: Some("proof-1"),
///         access_token: Some("tok-123"),
///     },
/// )
/// .expect("valid signing input");
///
/// let claims = verify_dpop_proof(&VerifyDpopProofInput {
///     proof: Some(&proof),
///     method: "POST",
///     url: "https://example.com/oauth/token",
///     now_epoch_seconds: 1_700_000_010,
///     expected_thumbprint: Some(&key_pair.thumbprint()),
///     expected_access_token: Some("tok-123"),
///     max_age_seconds: None,
/// })
/// .expect("valid DPoP proof");
/// assert_eq!(claims.jti, "proof-1");
/// ```
#[derive(Clone)]
pub struct DpopKeyPair {
    signing_key: SigningKey,
}

impl fmt::Debug for DpopKeyPair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DpopKeyPair")
            .field("thumbprint", &self.thumbprint())
            .finish_non_exhaustive()
    }
}

impl DpopKeyPair {
    /// Generates a fresh random P-256 key pair using the OS CSPRNG.
    pub fn generate() -> Self {
        Self {
            signing_key: SigningKey::random(&mut OsRng),
        }
    }

    /// Reconstructs a [`DpopKeyPair`] from a raw 32-byte big-endian P-256 private scalar.
    pub fn from_secret_bytes(bytes: &[u8]) -> Result<Self, DpopSignError> {
        let signing_key =
            SigningKey::from_slice(bytes).map_err(|_| DpopSignError::InvalidPrivateKey)?;
        Ok(Self { signing_key })
    }

    /// Returns the raw 32-byte big-endian private scalar.
    pub fn secret_bytes(&self) -> [u8; 32] {
        self.signing_key.to_bytes().into()
    }

    /// Exports the public coordinates as an EC P-256 [`DpopPublicJwk`].
    pub fn public_jwk(&self) -> DpopPublicJwk {
        let point = self.signing_key.verifying_key().to_encoded_point(false);
        let x = point.x().map_or(&[][..], |coord| &coord[..]);
        let y = point.y().map_or(&[][..], |coord| &coord[..]);
        DpopPublicJwk {
            kty: "EC".to_owned(),
            crv: "P-256".to_owned(),
            x: URL_SAFE_NO_PAD.encode(x),
            y: URL_SAFE_NO_PAD.encode(y),
        }
    }

    /// Computes the RFC 7638 SHA-256 thumbprint of this key pair's public JWK.
    pub fn thumbprint(&self) -> String {
        compute_dpop_jwk_thumbprint(&self.public_jwk())
    }
}

/// Parameters for creating a signed `DPoP` proof via [`sign_dpop_proof`].
#[derive(Debug, Clone, Copy)]
pub struct SignDpopProofInput<'a> {
    /// HTTP request method (`"GET"`, `"POST"`, etc.).
    pub method: &'a str,
    /// Target request URL (query and fragment are stripped automatically for `htu`).
    pub url: &'a str,
    /// Issued-at timestamp in Unix epoch seconds.
    pub iat_epoch_seconds: i64,
    /// Optional explicit token identifier; generates a random UUID v4 when `None`.
    pub jti: Option<&'a str>,
    /// Optional access token to bind via the RFC 9449 `ath` claim.
    pub access_token: Option<&'a str>,
}

/// Parameters for verifying an incoming `DPoP` proof via [`verify_dpop_proof`].
#[derive(Debug, Clone, Copy)]
pub struct VerifyDpopProofInput<'a> {
    /// Raw compact JWT proof string from the `DPoP` header / query parameter.
    pub proof: Option<&'a str>,
    /// Expected HTTP method (compared case-insensitively).
    pub method: &'a str,
    /// Target URL (normalized via [`normalize_dpop_htu`] before comparison).
    pub url: &'a str,
    /// Current verifier timestamp in Unix epoch seconds.
    pub now_epoch_seconds: i64,
    /// Optional expected RFC 7638 JWK thumbprint.
    pub expected_thumbprint: Option<&'a str>,
    /// Optional bound access token whose SHA-256 hash must match `ath`.
    pub expected_access_token: Option<&'a str>,
    /// Optional clock-skew limit in seconds; defaults to [`DPOP_MAX_CLOCK_SKEW_SECONDS`] (`300`).
    pub max_age_seconds: Option<i64>,
}

/// Verified `DPoP` proof claims returned on successful verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DpopVerifiedClaims {
    /// RFC 7638 SHA-256 base64url thumbprint of the embedded public JWK.
    pub thumbprint: String,
    /// Unique proof identifier (`jti`) for replay protection.
    pub jti: String,
    /// Proof issued-at timestamp (`iat`) in Unix epoch seconds.
    pub iat: i64,
}

#[derive(Serialize)]
struct DpopJwtHeader<'a> {
    typ: &'static str,
    alg: &'static str,
    jwk: &'a DpopPublicJwk,
}

#[derive(Serialize)]
struct DpopJwtPayload<'a> {
    htm: &'a str,
    htu: &'a str,
    jti: &'a str,
    iat: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    ath: Option<String>,
}

/// Computes the RFC 7638 canonical SHA-256 base64url thumbprint of an EC P-256 public JWK.
///
/// Serializes `{"crv":...,"kty":...,"x":...,"y":...}` in strict ASCII key order independent of
/// system locale, matching `computeDpopJwkThumbprint` in `dpop.ts`.
pub fn compute_dpop_jwk_thumbprint(jwk: &DpopPublicJwk) -> String {
    let mut canonical = String::with_capacity(160);
    canonical.push_str(r#"{"crv":"#);
    push_json_string(&mut canonical, &jwk.crv);
    canonical.push_str(r#","kty":"#);
    push_json_string(&mut canonical, &jwk.kty);
    canonical.push_str(r#","x":"#);
    push_json_string(&mut canonical, &jwk.x);
    canonical.push_str(r#","y":"#);
    push_json_string(&mut canonical, &jwk.y);
    canonical.push('}');

    let digest = Sha256::digest(canonical.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

/// Computes the RFC 9449 `ath` (access token hash) as unpadded base64url(SHA-256(access_token)).
pub fn compute_dpop_access_token_hash(access_token: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(access_token.as_bytes()))
}

/// Signs an ES256 `DPoP` compact JWT proof using `key_pair`.
pub fn sign_dpop_proof(
    key_pair: &DpopKeyPair,
    input: &SignDpopProofInput<'_>,
) -> Result<String, DpopSignError> {
    if input.method.trim().is_empty() {
        return Err(DpopSignError::EmptyMethod);
    }
    let normalized_htu = normalize_dpop_htu(input.url).ok_or(DpopSignError::InvalidUrl)?;
    let generated_jti;
    let jti = match input.jti {
        Some("") => return Err(DpopSignError::EmptyJti),
        Some(value) => value,
        None => {
            generated_jti = Uuid::new_v4().to_string();
            &generated_jti
        }
    };

    let public_jwk = key_pair.public_jwk();
    let header_json = serde_json::to_vec(&DpopJwtHeader {
        typ: DPOP_TYP,
        alg: DPOP_ALG,
        jwk: &public_jwk,
    })
    .map_err(|err| DpopSignError::Serialization(err.to_string()))?;

    let ath = input
        .access_token
        .filter(|tok| !tok.is_empty())
        .map(compute_dpop_access_token_hash);
    let payload_json = serde_json::to_vec(&DpopJwtPayload {
        htm: input.method,
        htu: &normalized_htu,
        jti,
        iat: input.iat_epoch_seconds,
        ath,
    })
    .map_err(|err| DpopSignError::Serialization(err.to_string()))?;

    let header_b64 = URL_SAFE_NO_PAD.encode(header_json);
    let payload_b64 = URL_SAFE_NO_PAD.encode(payload_json);
    let signing_input = format!("{header_b64}.{payload_b64}");

    let signature: Signature = key_pair.signing_key.sign(signing_input.as_bytes());
    let sig_b64 = URL_SAFE_NO_PAD.encode(signature.to_bytes());
    Ok(format!("{signing_input}.{sig_b64}"))
}

/// Verifies an incoming compact JWT `DPoP` proof against request metadata and clock skew bounds.
pub fn verify_dpop_proof(
    input: &VerifyDpopProofInput<'_>,
) -> Result<DpopVerifiedClaims, DpopVerificationError> {
    verify_dpop_proof_impl(input)
}
