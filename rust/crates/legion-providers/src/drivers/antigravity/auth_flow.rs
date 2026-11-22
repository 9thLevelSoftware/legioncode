//! Google OAuth URL validation, auth-flow error reset, and managed-release integrity checks.

use std::path::Path;

use serde::Deserialize;

const RELEASE_RECORD_NAME: &str = ".legioncode-release.json";
const MAX_RELEASE_RECORD_BYTES: u64 = 64 * 1024;

/// Phase tracked by Antigravity sign-in state machines.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AntigravityAuthFlowPhase {
    /// No active interactive sign-in flow is running.
    #[default]
    Idle,
    /// Interactive OAuth browser/code sign-in is awaiting user completion.
    Pending,
    /// Credentials verified for this instance.
    Authenticated,
    /// Prior sign-in attempt failed with an actionable error.
    Failed,
    /// Prior sign-in attempt was cancelled before completion.
    Cancelled,
}

/// Persisted sign-in UI status for an Antigravity provider instance.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AntigravityAuthState {
    /// Current flow phase.
    pub phase: AntigravityAuthFlowPhase,
    /// Last user-visible auth failure reason, if any.
    pub error: Option<String>,
}

/// Clears stale failed/cancelled sign-in errors once a session authenticates outside an
/// interactive login flow (`320ab2a2ac`).
#[must_use]
pub fn clear_stale_auth_error(
    state: AntigravityAuthState,
    in_active_auth_flow: bool,
) -> AntigravityAuthState {
    if in_active_auth_flow {
        return state;
    }
    match state.phase {
        AntigravityAuthFlowPhase::Failed | AntigravityAuthFlowPhase::Cancelled => {
            AntigravityAuthState {
                phase: AntigravityAuthFlowPhase::Idle,
                error: None,
            }
        }
        AntigravityAuthFlowPhase::Idle
        | AntigravityAuthFlowPhase::Pending
        | AntigravityAuthFlowPhase::Authenticated => state,
    }
}

/// Interactive Google OAuth flow variants emitted by `agy` CLI (`944d1820e2`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AntigravityOAuthKind {
    /// Local loopback server callback (`http://127.0.0.1:<port>/`).
    Loopback,
    /// Copy/paste authorization code callback (`https://antigravity.google/oauth-callback`).
    Code,
}

/// Validated pending Google OAuth authorization request (`944d1820e2`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AntigravityPendingAuth {
    /// Loopback or manual code OAuth flow.
    pub kind: AntigravityOAuthKind,
    /// Non-empty OAuth `state` nonce validated against responses.
    pub state: String,
    /// Normalized expected `redirect_uri` origin + path.
    pub redirect_uri: String,
}

/// Validates a Google OAuth URL emitted by Antigravity CLI (`944d1820e2`).
#[must_use]
pub fn parse_antigravity_authorization_url(
    raw_url: &str,
    kind: AntigravityOAuthKind,
) -> Option<AntigravityPendingAuth> {
    let trimmed = raw_url.trim();
    if trimmed.contains('#') {
        return None;
    }
    let rest = trimmed.strip_prefix("https://accounts.google.com")?;
    let expected_path = match kind {
        AntigravityOAuthKind::Loopback => "/o/oauth2/v2/auth?",
        AntigravityOAuthKind::Code => "/o/oauth2/auth?",
    };
    let query = rest.strip_prefix(expected_path)?;
    let state = single_query_param(query, "state")?;
    if state.is_empty() || state.len() > 512 || state.chars().any(char::is_whitespace) {
        return None;
    }
    let redirect = single_query_param(query, "redirect_uri")?;
    if !valid_redirect_uri(&redirect, kind) {
        return None;
    }
    Some(AntigravityPendingAuth {
        kind,
        state,
        redirect_uri: redirect,
    })
}

/// Extracts an authorization code from either a bare code or callback URL matching
/// `pending.redirect_uri` and `pending.state` (`944d1820e2`).
#[must_use]
pub fn validate_antigravity_login_response(
    pending: &AntigravityPendingAuth,
    response: &str,
) -> Option<String> {
    let trimmed = response.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.len() <= 2048
        && trimmed
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'/' | b'~' | b'-'))
    {
        return Some(trimmed.to_owned());
    }
    let (base, query) = trimmed.split_once('?')?;
    let query = query.split('#').next().unwrap_or(query);
    if base.contains('@') || base != pending.redirect_uri {
        return None;
    }
    let state = single_query_param(query, "state")?;
    if state != pending.state {
        return None;
    }
    let code = single_query_param(query, "code")?;
    (!code.is_empty()).then_some(code)
}

fn valid_redirect_uri(uri: &str, kind: AntigravityOAuthKind) -> bool {
    match kind {
        AntigravityOAuthKind::Code => uri == "https://antigravity.google/oauth-callback",
        AntigravityOAuthKind::Loopback => {
            let Some(port_part) = uri
                .strip_prefix("http://127.0.0.1:")
                .and_then(|s| s.strip_suffix('/'))
            else {
                return false;
            };
            !port_part.starts_with('0') && port_part.parse::<u16>().is_ok_and(|port| port >= 1024)
        }
    }
}

fn single_query_param(query: &str, key: &str) -> Option<String> {
    let mut found = None;
    for pair in query.split('&') {
        let (raw_k, raw_v) = pair.split_once('=')?;
        if percent_decode(raw_k)? == key {
            if found.is_some() {
                return None;
            }
            found = Some(percent_decode(raw_v)?);
        }
    }
    found
}

fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'+' {
            out.push(b' ');
            i += 1;
        } else if bytes[i] == b'%' {
            let hi = bytes.get(i + 1).copied()?;
            let lo = bytes.get(i + 2).copied()?;
            let decoded = hex::decode([hi, lo]).ok()?;
            out.push(decoded[0]);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Integrity state of an installed managed Antigravity release (`d1822be5a9`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagedReleaseIntegrity {
    /// Binary and release metadata match and are safe to execute.
    Valid {
        executable_path: String,
        version: String,
    },
    /// Corrupt, modified, pre-CLI, or incomplete managed release safe to reinstall.
    Repairable { message: String },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InstalledReleaseRecord {
    release_id: String,
    version: String,
    executable: InstalledExecutableRecord,
}

#[derive(Debug, Deserialize)]
struct InstalledExecutableRecord {
    name: String,
    bytes: u64,
}

/// Inspects a managed `versions/<release_id>` release on disk and marks incomplete,
/// modified, symlinked, or pre-CLI releases as [`ManagedReleaseIntegrity::Repairable`]
/// so reinstall recovery can repair them cleanly (`d1822be5a9`).
#[must_use]
pub fn inspect_managed_release(
    versions_dir: &Path,
    release_id: &str,
    expected_cli_executable_name: &str,
) -> ManagedReleaseIntegrity {
    let repairable = |msg: &str| ManagedReleaseIntegrity::Repairable {
        message: msg.to_owned(),
    };
    let dir = versions_dir.join(release_id);
    if is_symlink(&dir) || !dir.is_dir() {
        return repairable("The managed Antigravity runtime is incomplete. Reinstall it.");
    }
    let record_path = dir.join(RELEASE_RECORD_NAME);
    if is_symlink(&record_path) {
        return repairable("The managed Antigravity runtime is incomplete. Reinstall it.");
    }
    let Ok(record_meta) = std::fs::symlink_metadata(&record_path) else {
        return repairable("The managed Antigravity runtime is incomplete. Reinstall it.");
    };
    if !record_meta.is_file() || record_meta.len() > MAX_RELEASE_RECORD_BYTES {
        return repairable("The managed runtime record is invalid. Reinstall Antigravity.");
    }
    let Ok(raw_record) = std::fs::read_to_string(&record_path) else {
        return repairable("The managed runtime record is invalid. Reinstall Antigravity.");
    };
    let Ok(record) = serde_json::from_str::<InstalledReleaseRecord>(&raw_record) else {
        return repairable("The managed runtime record is invalid. Reinstall Antigravity.");
    };
    if record.release_id == release_id && record.executable.name != expected_cli_executable_name {
        return repairable(&format!(
            "Antigravity {} predates the Antigravity CLI and no longer works with this LegionCode release. Update it in Antigravity setup.",
            record.version
        ));
    }
    if record.release_id != release_id
        || record.executable.bytes == 0
        || record.version.trim().is_empty()
    {
        return repairable("The managed Antigravity runtime is incomplete. Reinstall it.");
    }
    let exec_path = dir.join(&record.executable.name);
    if is_symlink(&exec_path) {
        return repairable("The managed Antigravity runtime is incomplete. Reinstall it.");
    }
    let Ok(exec_meta) = std::fs::symlink_metadata(&exec_path) else {
        return repairable("The managed Antigravity runtime is incomplete. Reinstall it.");
    };
    if !exec_meta.is_file() || !is_executable_mode(&exec_meta) {
        return repairable("The managed Antigravity runtime is incomplete. Reinstall it.");
    }
    if exec_meta.len() != record.executable.bytes {
        return repairable(
            "The managed Antigravity runtime changed on disk. Reinstall it in Antigravity setup.",
        );
    }
    ManagedReleaseIntegrity::Valid {
        executable_path: exec_path.to_string_lossy().into_owned(),
        version: record.version,
    }
}

fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
}

#[cfg(unix)]
fn is_executable_mode(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    (meta.permissions().mode() & 0o111) != 0
}

#[cfg(not(unix))]
fn is_executable_mode(_meta: &std::fs::Metadata) -> bool {
    true
}
