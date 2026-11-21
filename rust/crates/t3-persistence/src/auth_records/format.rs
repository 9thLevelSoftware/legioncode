//! CLI text and JSON list formatters matching `apps/server/src/cliAuthFormat.ts:75-198`.

use serde::Serialize;

use super::{AuthRecordsError, AuthSessionRow, ClientMetadataRow, PairingLinkRow, json_err};

/// Formats active pairing credentials matching `cliAuthFormat.ts:75-111` byte-for-byte.
///
/// Every returned string ends with a single trailing `'\n'`, matching `cliAuthFormat.ts` (callers
/// writing to stdout should use `print!("{out}")` rather than `println!`).
///
/// # Examples
///
/// ```
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use t3_persistence::auth_records::format_pairing_credential_list;
///
/// assert_eq!(
///     format_pairing_credential_list(&[], false)?,
///     "No active pairing credentials.\n",
/// );
/// assert_eq!(format_pairing_credential_list(&[], true)?, "[]\n");
/// # Ok(())
/// # }
/// ```
pub fn format_pairing_credential_list(
    credentials: &[PairingLinkRow],
    json: bool,
) -> Result<String, AuthRecordsError> {
    if json {
        let payload: Vec<FormattedPairingLinkJson<'_>> = credentials
            .iter()
            .map(|c| FormattedPairingLinkJson {
                id: &c.id,
                label: c.label.as_deref().filter(|l| !l.is_empty()),
                scopes: &c.scopes,
                created_at: &c.created_at,
                expires_at: &c.expires_at,
            })
            .collect();
        let mut rendered = serde_json::to_string_pretty(&payload)
            .map_err(|source| json_err("format_pairing_credential_list", source))?;
        rendered.push('\n');
        return Ok(rendered);
    }

    if credentials.is_empty() {
        return Ok("No active pairing credentials.\n".to_owned());
    }

    let blocks: Vec<String> = credentials
        .iter()
        .map(|c| {
            let label_suffix = match c.label.as_deref().filter(|l| !l.is_empty()) {
                Some(label) => format!(" ({label})"),
                None => String::new(),
            };
            format!(
                "{}{}\n  scopes: {}\n  created: {}\n  expires: {}",
                c.id,
                label_suffix,
                c.scopes.join(" "),
                c.created_at,
                c.expires_at,
            )
        })
        .collect();
    let mut out = blocks.join("\n\n");
    out.push('\n');
    Ok(out)
}

/// Formats active sessions matching `cliAuthFormat.ts:152-198` byte-for-byte.
///
/// Every returned string ends with a single trailing `'\n'`, matching `cliAuthFormat.ts` (callers
/// writing to stdout should use `print!("{out}")` rather than `println!`).
///
/// # Examples
///
/// ```
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use t3_persistence::auth_records::format_session_list;
///
/// assert_eq!(format_session_list(&[], false)?, "No active sessions.\n");
/// assert_eq!(format_session_list(&[], true)?, "[]\n");
/// # Ok(())
/// # }
/// ```
pub fn format_session_list(
    sessions: &[AuthSessionRow],
    json: bool,
) -> Result<String, AuthRecordsError> {
    if json {
        let payload: Vec<FormattedSessionJson<'_>> = sessions
            .iter()
            .map(|s| FormattedSessionJson {
                session_id: &s.session_id,
                method: &s.method,
                scopes: &s.scopes,
                subject: &s.subject,
                client: &s.client,
                connected: s.connected,
                issued_at: &s.issued_at,
                expires_at: &s.expires_at,
                last_connected_at: s.last_connected_at.as_deref(),
            })
            .collect();
        let mut rendered = serde_json::to_string_pretty(&payload)
            .map_err(|source| json_err("format_session_list", source))?;
        rendered.push('\n');
        return Ok(rendered);
    }

    if sessions.is_empty() {
        return Ok("No active sessions.\n".to_owned());
    }

    let blocks: Vec<String> = sessions
        .iter()
        .map(|s| {
            let connected = if s.connected { " connected" } else { "" };
            let last_conn = s.last_connected_at.as_deref().unwrap_or("never");
            format!(
                "{}{}\n  scopes: {}\n  method: {}\n  subject: {}\n  client: {}\n  issued: {}\n  last connected: {}\n  expires: {}",
                s.session_id,
                connected,
                s.scopes.join(" "),
                s.method,
                s.subject,
                format_client_metadata(&s.client),
                s.issued_at,
                last_conn,
                s.expires_at,
            )
        })
        .collect();
    let mut out = blocks.join("\n\n");
    out.push('\n');
    Ok(out)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FormattedPairingLinkJson<'a> {
    id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<&'a str>,
    scopes: &'a [String],
    created_at: &'a str,
    expires_at: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FormattedSessionJson<'a> {
    session_id: &'a str,
    method: &'a str,
    scopes: &'a [String],
    subject: &'a str,
    client: &'a ClientMetadataRow,
    connected: bool,
    issued_at: &'a str,
    expires_at: &'a str,
    last_connected_at: Option<&'a str>,
}

pub(crate) fn format_client_metadata(metadata: &ClientMetadataRow) -> String {
    let details: Vec<&str> = [
        metadata.label.as_deref(),
        (metadata.device_type != "unknown").then_some(metadata.device_type.as_str()),
        metadata.os.as_deref(),
        metadata.browser.as_deref(),
        metadata.ip_address.as_deref(),
    ]
    .into_iter()
    .flatten()
    .filter(|value| !value.is_empty())
    .collect();
    if details.is_empty() {
        "unlabeled client".to_owned()
    } else {
        details.join(" | ")
    }
}
