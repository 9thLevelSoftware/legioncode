//! Legion Connect waiting pairing-link approvals, trusted device queries, verification codes,
//! and duplicate-code ambiguity guards (`auth.ts:230-233`).
//!
//! Mirrors `apps/server/src/cli/auth.ts`, `apps/server/src/auth/EnvironmentAuth.ts`,
//! `apps/server/src/auth/PairingGrantStore.ts`, and
//! `apps/server/src/persistence/AuthConnectDevices.ts`.
//!
//! # Examples
//!
//! ```
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use legion_persistence::auth_connect_devices::{
//!     connect_device_verification_code, format_connect_device_list, list_connect_devices,
//! };
//! use legion_persistence::sqlite::SqliteStore;
//!
//! let code = connect_device_verification_code("AbCdEf12");
//! assert_eq!(code.len(), 9);
//! let store = SqliteStore::open_in_memory()?;
//! let (waiting, trusted) = list_connect_devices(store.connection())?;
//! assert_eq!(
//!     format_connect_device_list(&waiting, &trusted, false)?,
//!     "No devices are waiting for approval.\n\nNo trusted Legion Connect devices.\n",
//! );
//! # Ok(())
//! # }
//! ```

mod format;
mod queries;

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::auth_mutations::{TxGuard, current_epoch_ms, ensure_schema, revoke_connect_device_tx};
use crate::auth_records::{AuthRecordsError, ClientMetadataRow};
use crate::ownership::format_epoch_millis_utc;
use crate::pairing_tokens::PAIRING_CODE_ALPHABET;

pub use format::format_connect_device_list;
use queries::{WaitingLinkRow, query_trusted_devices, query_waiting_links, sqlite_err};

const BASE64URL_ALPHABET: &[u8] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Waiting Legion Connect approval request derived from an active `auth_pairing_links` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WaitingConnectDevice {
    /// Owning `auth_pairing_links.id` (internal key omitted from wire JSON).
    #[serde(skip)]
    pub pairing_link_id: String,
    /// Human-comparable `XXXX-XXXX` verification code.
    pub verification_code: String,
    /// Optional client metadata reported on the pending request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<ClientMetadataRow>,
}

/// Trusted Legion Connect device stored in `auth_connect_devices`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustedConnectDevice {
    /// RFC 7638 JWK SHA-256 thumbprint.
    pub proof_key_thumbprint: String,
    /// Human-comparable `XXXX-XXXX` verification code.
    pub verification_code: String,
    /// Optional device metadata recorded when approved or used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<ClientMetadataRow>,
    /// ISO-8601 UTC approval timestamp.
    pub approved_at: String,
}

/// Result of [`approve_connect_device_by_code`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectApproveOutcome {
    /// Exactly one waiting request matched and was trusted.
    Approved {
        /// Approved `auth_pairing_links.id`.
        pairing_link_id: String,
        /// Trusted device `proof_key_thumbprint`.
        proof_key_thumbprint: String,
        /// Normalized `XXXX-XXXX` verification code.
        verification_code: String,
    },
    /// No active waiting request matched `code`.
    NotFound,
    /// Multiple waiting requests shared `code`; neither was approved (`auth.ts:230-233`).
    Ambiguous {
        /// Count of waiting requests sharing `code`.
        match_count: usize,
    },
}

/// Result of [`revoke_connect_device_by_code`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectRevokeOutcome {
    /// Exactly one trusted device matched and was revoked alongside its sessions and unspent links.
    Revoked {
        /// Revoked device `proof_key_thumbprint`.
        proof_key_thumbprint: String,
    },
    /// No trusted device matched `code`.
    NotFound,
    /// Multiple trusted devices shared `code`; neither was revoked (`auth.ts:263`).
    Ambiguous {
        /// Count of trusted devices sharing `code`.
        match_count: usize,
    },
}

/// Computes the deterministic `XXXX-XXXX` verification code from `proof_key_thumbprint`
/// matching `packages/contracts/src/auth.ts:343-350`.
pub fn connect_device_verification_code(proof_key_thumbprint: &str) -> String {
    let alphabet = PAIRING_CODE_ALPHABET.as_bytes();
    let mut code = String::with_capacity(8);
    for ch in proof_key_thumbprint.chars().take(8) {
        let byte_pos = u8::try_from(ch)
            .ok()
            .and_then(|b| BASE64URL_ALPHABET.iter().position(|&c| c == b))
            .unwrap_or(0);
        code.push(char::from(alphabet[byte_pos % alphabet.len()]));
    }
    let prefix: String = code.chars().take(4).collect();
    let suffix: String = code.chars().skip(4).collect();
    format!("{prefix}-{suffix}")
}

/// Strips non-alphanumeric separators and uppercases verification codes (`EnvironmentAuth.ts:1694`).
pub fn normalize_verification_code(code: &str) -> String {
    code.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|ch| ch.to_ascii_uppercase())
        .collect()
}

/// Lists active waiting Connect requests and trusted Connect devices (`cli/auth.ts:204-208`).
pub fn list_connect_devices(
    conn: &Connection,
) -> Result<(Vec<WaitingConnectDevice>, Vec<TrustedConnectDevice>), AuthRecordsError> {
    list_connect_devices_at(conn, current_epoch_ms())
}

/// Deterministic timestamp variant of [`list_connect_devices`].
pub fn list_connect_devices_at(
    conn: &Connection,
    now_ms: i64,
) -> Result<(Vec<WaitingConnectDevice>, Vec<TrustedConnectDevice>), AuthRecordsError> {
    let now_iso = format_epoch_millis_utc(now_ms);
    let waiting = query_waiting_links(conn, &now_iso)?
        .into_iter()
        .map(|row| WaitingConnectDevice {
            pairing_link_id: row.pairing_link_id,
            verification_code: connect_device_verification_code(&row.proof_key_thumbprint),
            client: row.client,
        })
        .collect();
    let trusted = query_trusted_devices(conn)?;
    Ok((waiting, trusted))
}

/// Approves the single waiting Connect device matching `code` (`cli/auth.ts:227-240`).
///
/// Refuses to approve either request when multiple waiting devices share `code`.
pub fn approve_connect_device_by_code(
    conn: &Connection,
    code: &str,
) -> Result<ConnectApproveOutcome, AuthRecordsError> {
    approve_connect_device_by_code_at(conn, code, current_epoch_ms())
}

/// Deterministic timestamp variant of [`approve_connect_device_by_code`].
pub fn approve_connect_device_by_code_at(
    conn: &Connection,
    code: &str,
    now_ms: i64,
) -> Result<ConnectApproveOutcome, AuthRecordsError> {
    ensure_schema(conn)?;
    let wanted = normalize_verification_code(code);
    if wanted.is_empty() {
        return Ok(ConnectApproveOutcome::NotFound);
    }

    let tx = TxGuard::begin(conn)?;
    let now_iso = format_epoch_millis_utc(now_ms);
    let matches: Vec<WaitingLinkRow> = query_waiting_links(conn, &now_iso)?
        .into_iter()
        .filter(|row| {
            normalize_verification_code(&connect_device_verification_code(
                &row.proof_key_thumbprint,
            )) == wanted
        })
        .collect();

    if matches.is_empty() {
        return Ok(ConnectApproveOutcome::NotFound);
    }
    if matches.len() > 1 {
        return Ok(ConnectApproveOutcome::Ambiguous {
            match_count: matches.len(),
        });
    }

    let chosen = &matches[0];
    let verification_code = connect_device_verification_code(&chosen.proof_key_thumbprint);
    let client_json = match &chosen.client {
        Some(client) => {
            Some(
                serde_json::to_string(client).map_err(|source| AuthRecordsError::Json {
                    operation: "approve_connect_device:client_json",
                    source,
                })?,
            )
        }
        None => None,
    };

    conn.execute(
        "INSERT INTO auth_connect_devices (proof_key_thumbprint, client_json, approved_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT (proof_key_thumbprint) DO UPDATE
           SET client_json = COALESCE(excluded.client_json, auth_connect_devices.client_json)",
        params![chosen.proof_key_thumbprint, client_json, now_iso],
    )
    .map_err(|source| sqlite_err("approve_connect_device:trust", source))?;

    tx.commit()?;
    Ok(ConnectApproveOutcome::Approved {
        pairing_link_id: chosen.pairing_link_id.clone(),
        proof_key_thumbprint: chosen.proof_key_thumbprint.clone(),
        verification_code,
    })
}

/// Revokes the single trusted Connect device matching `code`, cascading to unspent pairing links
/// and active sessions (`cli/auth.ts:259-275`, `EnvironmentAuth.ts:1306-1318`).
pub fn revoke_connect_device_by_code(
    conn: &Connection,
    code: &str,
) -> Result<ConnectRevokeOutcome, AuthRecordsError> {
    revoke_connect_device_by_code_at(conn, code, current_epoch_ms())
}

/// Deterministic timestamp variant of [`revoke_connect_device_by_code`].
pub fn revoke_connect_device_by_code_at(
    conn: &Connection,
    code: &str,
    now_ms: i64,
) -> Result<ConnectRevokeOutcome, AuthRecordsError> {
    ensure_schema(conn)?;
    let wanted = normalize_verification_code(code);
    if wanted.is_empty() {
        return Ok(ConnectRevokeOutcome::NotFound);
    }

    let tx = TxGuard::begin(conn)?;
    let matches: Vec<TrustedConnectDevice> = query_trusted_devices(conn)?
        .into_iter()
        .filter(|dev| normalize_verification_code(&dev.verification_code) == wanted)
        .collect();

    if matches.is_empty() {
        return Ok(ConnectRevokeOutcome::NotFound);
    }
    if matches.len() > 1 {
        return Ok(ConnectRevokeOutcome::Ambiguous {
            match_count: matches.len(),
        });
    }

    let thumbprint = matches[0].proof_key_thumbprint.clone();
    let now_iso = format_epoch_millis_utc(now_ms);
    revoke_connect_device_tx(conn, &thumbprint, &now_iso)?;
    tx.commit()?;
    Ok(ConnectRevokeOutcome::Revoked {
        proof_key_thumbprint: thumbprint,
    })
}
