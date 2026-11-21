//! CLI duration parsing (`DurationFromString`) and issued credential/session formatters.

use std::fmt::Write as _;

use serde::Serialize;

use crate::auth_records::{
    AuthRecordsError, ClientMetadataRow, PairingLinkRow, format_client_metadata,
};

use super::IssuedSession;

const DURATION_ERR: &str = "Invalid duration. Use values like 5m, 1h, 30d, or 15 minutes.";

/// Parses a CLI duration string (`"5m"`, `"1h"`, `"30d"`, `"15 minutes"`, `"500ms"`, etc.)
/// matching `DurationFromString` in `apps/server/src/cli/config.ts:569-630`.
pub fn parse_cli_duration_ms(raw: &str) -> Result<u64, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(DURATION_ERR.to_owned());
    }

    let split_idx = trimmed
        .find(|ch: char| !ch.is_ascii_digit() && ch != '.')
        .ok_or_else(|| DURATION_ERR.to_owned())?;
    let (num_raw, unit_raw) = trimmed.split_at(split_idx);
    let unit = unit_raw.trim();
    if num_raw.is_empty() || unit.is_empty() {
        return Err(DURATION_ERR.to_owned());
    }

    let factor_ms: u64 = match unit.to_ascii_lowercase().as_str() {
        "ms" | "milli" | "millis" | "millisecond" | "milliseconds" => 1,
        "s" | "sec" | "secs" | "second" | "seconds" => 1_000,
        "m" | "min" | "mins" | "minute" | "minutes" => 60_000,
        "h" | "hr" | "hrs" | "hour" | "hours" => 3_600_000,
        "d" | "day" | "days" => 86_400_000,
        "w" | "wk" | "wks" | "week" | "weeks" => 604_800_000,
        _ => return Err(DURATION_ERR.to_owned()),
    };

    if !num_raw.contains('.') {
        let amount = num_raw
            .parse::<u64>()
            .map_err(|_| DURATION_ERR.to_owned())?;
        return amount
            .checked_mul(factor_ms)
            .ok_or_else(|| DURATION_ERR.to_owned());
    }

    // Decimal shorthand like "1.5h" is rejected by DurationShorthandPattern; allow spaced "1.5 hours".
    if !unit_raw.starts_with(char::is_whitespace) {
        return Err(DURATION_ERR.to_owned());
    }
    let amount = num_raw
        .parse::<f64>()
        .map_err(|_| DURATION_ERR.to_owned())?;
    if !amount.is_finite() || amount < 0.0 {
        return Err(DURATION_ERR.to_owned());
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "u64 multiplier <= 604_800_000 fits exactly in f64; bounded before integer cast"
    )]
    let total = (amount * factor_ms as f64).round();
    #[expect(
        clippy::cast_precision_loss,
        reason = "finite non-negative total checked against u64::MAX"
    )]
    if !total.is_finite() || total < 0.0 || total > u64::MAX as f64 {
        Err(DURATION_ERR.to_owned())
    } else {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "bounds verified immediately above"
        )]
        Ok(total as u64)
    }
}

/// Formats an issued client pairing credential matching `cliAuthFormat.ts:33-73` byte-for-byte.
pub fn format_issued_pairing_credential(
    credential: &PairingLinkRow,
    json: bool,
    base_url: Option<&str>,
) -> Result<String, AuthRecordsError> {
    let pair_url = base_url
        .filter(|url| !url.is_empty())
        .and_then(|url| build_pair_url(url, &credential.credential));

    if json {
        let payload = IssuedPairingJson {
            id: &credential.id,
            credential: &credential.credential,
            label: credential.label.as_deref().filter(|l| !l.is_empty()),
            scopes: &credential.scopes,
            expires_at: &credential.expires_at,
            pair_url: pair_url.as_deref(),
        };
        let mut out =
            serde_json::to_string_pretty(&payload).map_err(|source| AuthRecordsError::Json {
                operation: "format_issued_pairing_credential",
                source,
            })?;
        out.push('\n');
        return Ok(out);
    }

    let mut lines = vec![
        format!("Issued client pairing token {}.", credential.id),
        format!("Token: {}", credential.credential),
    ];
    if let Some(url) = pair_url {
        lines.push(format!("Pair URL: {url}"));
    }
    lines.push(format!("Expires at: {}", credential.expires_at));
    let mut out = lines.join("\n");
    out.push('\n');
    Ok(out)
}

/// Formats an issued bearer access token session matching `cliAuthFormat.ts:113-150` byte-for-byte.
pub fn format_issued_session(
    session: &IssuedSession,
    json: bool,
    token_only: bool,
) -> Result<String, AuthRecordsError> {
    if token_only {
        return Ok(format!("{}\n", session.token));
    }

    if json {
        let payload = IssuedSessionJson {
            session_id: &session.session_id,
            token: &session.token,
            method: &session.method,
            scopes: &session.scopes,
            subject: &session.subject,
            client: &session.client,
            expires_at: &session.expires_at,
        };
        let mut out =
            serde_json::to_string_pretty(&payload).map_err(|source| AuthRecordsError::Json {
                operation: "format_issued_session",
                source,
            })?;
        out.push('\n');
        return Ok(out);
    }

    let lines = [
        format!("Issued bearer access token {}.", session.session_id),
        format!("Scopes: {}", session.scopes.join(" ")),
        format!("Token: {}", session.token),
        format!("Subject: {}", session.subject),
        format!("Client: {}", format_client_metadata(&session.client)),
        format!("Expires at: {}", session.expires_at),
    ];
    let mut out = lines.join("\n");
    out.push('\n');
    Ok(out)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct IssuedPairingJson<'a> {
    id: &'a str,
    credential: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<&'a str>,
    scopes: &'a [String],
    expires_at: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pair_url: Option<&'a str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct IssuedSessionJson<'a> {
    session_id: &'a str,
    token: &'a str,
    method: &'a str,
    scopes: &'a [String],
    subject: &'a str,
    client: &'a ClientMetadataRow,
    expires_at: &'a str,
}

fn build_pair_url(base_url: &str, token: &str) -> Option<String> {
    let scheme_end = base_url.find("://")?;
    let rest = &base_url[scheme_end + 3..];
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let origin = &base_url[..scheme_end + 3 + authority_end];
    if origin.len() <= scheme_end + 3 {
        return None;
    }

    let mut out = format!("{origin}/pair#token=");
    for byte in token.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(*byte));
        } else if *byte == b' ' {
            out.push('+');
        } else {
            let _ = write!(&mut out, "%{byte:02X}");
        }
    }
    Some(out)
}
