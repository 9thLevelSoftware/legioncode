//! Verification helpers for incoming ES256 `DPoP` JWT proofs.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use p256::ecdsa::signature::Verifier as _;
use p256::ecdsa::{Signature, VerifyingKey};
use serde::Deserialize;

use crate::dpop::{
    DPOP_ALG, DPOP_MAX_CLOCK_SKEW_SECONDS, DPOP_TYP, DpopPublicJwk, DpopVerificationError,
    DpopVerificationFailureCode, DpopVerifiedClaims, VerifyDpopProofInput,
    compute_dpop_access_token_hash, compute_dpop_jwk_thumbprint,
};
use crate::dpop_htu::normalize_dpop_htu;

#[derive(Deserialize)]
struct RawHeaderJwk {
    kty: String,
    crv: String,
    x: String,
    y: String,
    #[serde(default)]
    d: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct RawJwtHeader {
    typ: String,
    alg: String,
    jwk: RawHeaderJwk,
}

#[derive(Deserialize)]
struct RawJwtPayload {
    htm: String,
    htu: String,
    jti: String,
    iat: i64,
    #[serde(default)]
    ath: Option<String>,
}

pub(crate) fn verify_dpop_proof_impl(
    input: &VerifyDpopProofInput<'_>,
) -> Result<DpopVerifiedClaims, DpopVerificationError> {
    let Some(raw_proof) = input.proof.map(str::trim).filter(|s| !s.is_empty()) else {
        return Err(DpopVerificationError::new(
            DpopVerificationFailureCode::MissingProof,
            "Missing DPoP proof.",
        ));
    };

    let mut split = raw_proof.split('.');
    let (Some(header_b64), Some(payload_b64), Some(sig_b64), None) =
        (split.next(), split.next(), split.next(), split.next())
    else {
        return Err(DpopVerificationError::new(
            DpopVerificationFailureCode::MalformedProof,
            "Invalid DPoP compact JWT.",
        ));
    };
    if header_b64.is_empty() || payload_b64.is_empty() || sig_b64.is_empty() {
        return Err(DpopVerificationError::new(
            DpopVerificationFailureCode::MalformedProof,
            "Invalid DPoP compact JWT.",
        ));
    }

    let header = parse_header(header_b64)?;
    let payload = parse_payload(payload_b64)?;
    let thumbprint = compute_dpop_jwk_thumbprint(&header);

    if let Some(expected) = input.expected_thumbprint.filter(|s| !s.is_empty())
        && thumbprint != expected
    {
        return Err(DpopVerificationError::new(
            DpopVerificationFailureCode::KeyMismatch,
            "DPoP key thumbprint mismatch.",
        ));
    }
    if !payload.htm.eq_ignore_ascii_case(input.method) {
        return Err(DpopVerificationError::new(
            DpopVerificationFailureCode::MethodMismatch,
            "DPoP method mismatch.",
        ));
    }

    let Some(expected_htu) = normalize_dpop_htu(input.url) else {
        return Err(DpopVerificationError::new(
            DpopVerificationFailureCode::UrlMismatch,
            "DPoP URL mismatch.",
        ));
    };
    if payload.htu != expected_htu {
        return Err(DpopVerificationError::new(
            DpopVerificationFailureCode::UrlMismatch,
            "DPoP URL mismatch.",
        ));
    }

    if let Some(expected_token) = input.expected_access_token.filter(|s| !s.is_empty()) {
        let expected_ath = compute_dpop_access_token_hash(expected_token);
        if payload.ath.as_deref() != Some(expected_ath.as_str()) {
            return Err(DpopVerificationError::new(
                DpopVerificationFailureCode::AccessTokenHashMismatch,
                "DPoP access token hash mismatch.",
            ));
        }
    }

    let verifying_key = verifying_key_from_jwk(&header)?;
    let signature = parse_signature(sig_b64)?;
    let signing_input = format!("{header_b64}.{payload_b64}");
    if verifying_key
        .verify(signing_input.as_bytes(), &signature)
        .is_err()
    {
        return Err(DpopVerificationError::new(
            DpopVerificationFailureCode::InvalidSignature,
            "Invalid DPoP signature.",
        ));
    }

    let max_age = input.max_age_seconds.unwrap_or(DPOP_MAX_CLOCK_SKEW_SECONDS);
    let skew = input.now_epoch_seconds.abs_diff(payload.iat);
    if max_age < 0 || skew > max_age.unsigned_abs() {
        return Err(DpopVerificationError::new(
            DpopVerificationFailureCode::TimeWindow,
            "DPoP proof is outside the allowed time window.",
        ));
    }

    Ok(DpopVerifiedClaims {
        thumbprint,
        jti: payload.jti,
        iat: payload.iat,
    })
}

fn parse_header(header_b64: &str) -> Result<DpopPublicJwk, DpopVerificationError> {
    let bytes = URL_SAFE_NO_PAD.decode(header_b64).map_err(|_| {
        DpopVerificationError::new(
            DpopVerificationFailureCode::InvalidProof,
            "Invalid DPoP proof.",
        )
    })?;
    let json_str = std::str::from_utf8(&bytes).map_err(|_| {
        DpopVerificationError::new(
            DpopVerificationFailureCode::InvalidProof,
            "Invalid DPoP proof.",
        )
    })?;
    let raw: RawJwtHeader = serde_json::from_str(json_str).map_err(|_| {
        DpopVerificationError::new(
            DpopVerificationFailureCode::MalformedProof,
            "Invalid DPoP JWT header.",
        )
    })?;
    if raw.typ != DPOP_TYP
        || raw.alg != DPOP_ALG
        || raw.jwk.kty != "EC"
        || raw.jwk.crv != "P-256"
        || raw.jwk.x.is_empty()
        || raw.jwk.y.is_empty()
        || raw.jwk.d.is_some()
    {
        return Err(DpopVerificationError::new(
            DpopVerificationFailureCode::MalformedProof,
            "Invalid DPoP JWT header.",
        ));
    }
    Ok(DpopPublicJwk {
        kty: raw.jwk.kty,
        crv: raw.jwk.crv,
        x: raw.jwk.x,
        y: raw.jwk.y,
    })
}

fn parse_payload(payload_b64: &str) -> Result<RawJwtPayload, DpopVerificationError> {
    let bytes = URL_SAFE_NO_PAD.decode(payload_b64).map_err(|_| {
        DpopVerificationError::new(
            DpopVerificationFailureCode::InvalidProof,
            "Invalid DPoP proof.",
        )
    })?;
    let json_str = std::str::from_utf8(&bytes).map_err(|_| {
        DpopVerificationError::new(
            DpopVerificationFailureCode::InvalidProof,
            "Invalid DPoP proof.",
        )
    })?;
    let raw: RawJwtPayload = serde_json::from_str(json_str).map_err(|_| {
        DpopVerificationError::new(
            DpopVerificationFailureCode::MalformedProof,
            "Invalid DPoP JWT payload.",
        )
    })?;
    if raw.htm.is_empty() || raw.htu.is_empty() || raw.jti.is_empty() {
        return Err(DpopVerificationError::new(
            DpopVerificationFailureCode::MalformedProof,
            "Invalid DPoP JWT payload.",
        ));
    }
    Ok(raw)
}

fn verifying_key_from_jwk(jwk: &DpopPublicJwk) -> Result<VerifyingKey, DpopVerificationError> {
    let x = URL_SAFE_NO_PAD.decode(&jwk.x).map_err(|_| {
        DpopVerificationError::new(
            DpopVerificationFailureCode::InvalidProof,
            "Invalid DPoP proof.",
        )
    })?;
    let y = URL_SAFE_NO_PAD.decode(&jwk.y).map_err(|_| {
        DpopVerificationError::new(
            DpopVerificationFailureCode::InvalidProof,
            "Invalid DPoP proof.",
        )
    })?;
    if x.len() != 32 || y.len() != 32 {
        return Err(DpopVerificationError::new(
            DpopVerificationFailureCode::InvalidProof,
            "Invalid DPoP proof.",
        ));
    }
    let mut sec1 = [0u8; 65];
    sec1[0] = 0x04;
    sec1[1..33].copy_from_slice(&x);
    sec1[33..65].copy_from_slice(&y);
    VerifyingKey::from_sec1_bytes(&sec1).map_err(|_| {
        DpopVerificationError::new(
            DpopVerificationFailureCode::InvalidProof,
            "Invalid DPoP proof.",
        )
    })
}

fn parse_signature(sig_b64: &str) -> Result<Signature, DpopVerificationError> {
    let bytes = URL_SAFE_NO_PAD.decode(sig_b64).map_err(|_| {
        DpopVerificationError::new(
            DpopVerificationFailureCode::InvalidProof,
            "Invalid DPoP proof.",
        )
    })?;
    if bytes.len() != 64 {
        return Err(DpopVerificationError::new(
            DpopVerificationFailureCode::InvalidProof,
            "Invalid DPoP proof.",
        ));
    }
    Signature::from_slice(&bytes).map_err(|_| {
        DpopVerificationError::new(
            DpopVerificationFailureCode::InvalidSignature,
            "Invalid DPoP signature.",
        )
    })
}
