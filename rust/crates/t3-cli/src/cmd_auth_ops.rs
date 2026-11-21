//! SQLite and live-server operations for `t3 auth pairing` and `t3 auth session`.

use t3_persistence::auth_mutations::{
    IssueSessionInput, format_issued_pairing_credential, format_issued_session,
    issue_scoped_session, revoke_pairing_link, revoke_session,
};
use t3_persistence::auth_records::{
    PairingLinkRow, format_pairing_credential_list, format_session_list, list_active_pairing_links,
    list_active_sessions,
};
use t3_persistence::ownership::{format_epoch_millis_utc, read_live_server_runtime_state};
use t3_persistence::pairing_tokens::{
    DEFAULT_PAIRING_TOKEN_TTL_MS, IssuePairingTokenInput, STANDARD_CLIENT_SCOPES,
    issue_pairing_token_with_conn,
};
use t3_persistence::paths::{ServerDerivedPaths, ensure_server_directories};
use t3_persistence::sqlite::SqliteStore;

use crate::cmd_auth_args::{AuthCommonFlags, AuthOutputMode};
use crate::cmd_auth_live::try_live_auth_delete;
use crate::cmd_trace::current_epoch_ms;
use crate::fallback::{resolve_base_dir, write_stdout_line, write_stdout_raw};

pub(crate) fn derive_auth_paths(
    base_dir: Option<&str>,
    dev_url: Option<&str>,
) -> ServerDerivedPaths {
    let (resolved, is_explicit) = resolve_base_dir(base_dir);
    ServerDerivedPaths::derive(&resolved, dev_url.is_some(), is_explicit)
}

pub(crate) fn open_auth_store(paths: &ServerDerivedPaths) -> Result<SqliteStore, i32> {
    if let Err(err) = ensure_server_directories(paths) {
        eprintln!("Failed to prepare auth directories: {err}");
        return Err(1);
    }
    SqliteStore::open(&paths.db_path).map_err(|err| {
        eprintln!(
            "Failed to open auth database `{}`: {err}",
            paths.db_path.display()
        );
        1
    })
}

pub(crate) fn exec_pairing_create(flags: &AuthCommonFlags) -> i32 {
    let paths = derive_auth_paths(flags.base_dir.as_deref(), flags.dev_url.as_deref());
    let Ok(store) = open_auth_store(&paths) else {
        return 1;
    };
    let effective_scopes: Vec<String> = if flags.scopes.is_empty() {
        STANDARD_CLIENT_SCOPES
            .iter()
            .map(|s| (*s).to_owned())
            .collect()
    } else {
        flags.scopes.clone()
    };
    let ttl_i64 = flags
        .ttl_ms
        .and_then(|ms| i64::try_from(ms).ok())
        .filter(|&ms| ms > 0)
        .unwrap_or(DEFAULT_PAIRING_TOKEN_TTL_MS);
    let mut input = IssuePairingTokenInput::default()
        .with_scopes(effective_scopes.clone())
        .with_ttl_ms(ttl_i64);
    if let Some(lbl) = flags.label.as_deref() {
        input = input.with_label(lbl);
    }

    let now_ms = current_epoch_ms();
    let issued = match issue_pairing_token_with_conn(store.connection(), &input, now_ms) {
        Ok(val) => val,
        Err(err) => {
            eprintln!("Failed to create pairing credential: {err}");
            return 1;
        }
    };
    let row = PairingLinkRow {
        id: issued.id,
        credential: issued.credential,
        method: "one-time-token".to_owned(),
        scopes: effective_scopes,
        subject: "one-time-token".to_owned(),
        label: flags.label.clone(),
        created_at: format_epoch_millis_utc(now_ms),
        expires_at: issued.expires_at_iso,
        consumed_at: None,
        revoked_at: None,
        proof_key_thumbprint: None,
    };
    let is_json = flags.output_mode == AuthOutputMode::Json;
    match format_issued_pairing_credential(&row, is_json, flags.base_url.as_deref()) {
        Ok(rendered) => emit_formatted(&rendered, is_json),
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}

pub(crate) fn exec_pairing_list(base_dir: Option<&str>, dev_url: Option<&str>, json: bool) -> i32 {
    let paths = derive_auth_paths(base_dir, dev_url);
    let Ok(store) = open_auth_store(&paths) else {
        return 1;
    };
    let now_iso = format_epoch_millis_utc(current_epoch_ms());
    let links = match list_active_pairing_links(store.connection(), &now_iso) {
        Ok(rows) => rows,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };
    match format_pairing_credential_list(&links, json) {
        Ok(rendered) => write_stdout_line(&rendered, "auth output"),
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}

pub(crate) fn exec_pairing_revoke(base_dir: Option<&str>, dev_url: Option<&str>, id: &str) -> i32 {
    let paths = derive_auth_paths(base_dir, dev_url);
    if let Some(runtime) = read_live_server_runtime_state(&paths.server_runtime_state_path)
        && let Some(code) = try_live_auth_delete(
            &paths,
            &runtime.origin,
            &format!("/api/auth/pairing/{id}"),
            &format!("Revoked pairing credential {id}.\n"),
            &format!("No active pairing credential found for {id}.\n"),
        )
    {
        return code;
    }

    let Ok(store) = open_auth_store(&paths) else {
        return 1;
    };
    match revoke_pairing_link(store.connection(), id) {
        Ok(true) => write_stdout_line(&format!("Revoked pairing credential {id}.\n"), "auth"),
        Ok(false) => write_stdout_line(
            &format!("No active pairing credential found for {id}.\n"),
            "auth",
        ),
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}

pub(crate) fn exec_session_issue(flags: &AuthCommonFlags) -> i32 {
    let paths = derive_auth_paths(flags.base_dir.as_deref(), flags.dev_url.as_deref());
    let Ok(store) = open_auth_store(&paths) else {
        return 1;
    };
    let mut input = IssueSessionInput::default();
    if !flags.scopes.is_empty() {
        input = input.with_scopes(flags.scopes.iter().cloned());
    }
    if let Some(ms) = flags.ttl_ms {
        input = input.with_ttl_ms(ms);
    }
    if let Some(lbl) = flags.label.as_deref() {
        input = input.with_label(lbl);
    }
    if let Some(subj) = flags.subject.as_deref() {
        input = input.with_subject(subj);
    }

    let issued = match issue_scoped_session(store.connection(), input) {
        Ok(val) => val,
        Err(err) => {
            eprintln!("Failed to issue bearer session: {err}");
            return 1;
        }
    };
    let is_json = flags.output_mode == AuthOutputMode::Json;
    let token_only = flags.output_mode == AuthOutputMode::TokenOnly;
    match format_issued_session(&issued, is_json, token_only) {
        Ok(rendered) => emit_formatted(&rendered, is_json || token_only),
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}

pub(crate) fn exec_session_list(base_dir: Option<&str>, dev_url: Option<&str>, json: bool) -> i32 {
    let paths = derive_auth_paths(base_dir, dev_url);
    let Ok(store) = open_auth_store(&paths) else {
        return 1;
    };
    let now_iso = format_epoch_millis_utc(current_epoch_ms());
    let sessions = match list_active_sessions(store.connection(), &now_iso) {
        Ok(rows) => rows,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };
    match format_session_list(&sessions, json) {
        Ok(rendered) => write_stdout_line(&rendered, "auth output"),
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}

pub(crate) fn exec_session_revoke(
    base_dir: Option<&str>,
    dev_url: Option<&str>,
    session_id: &str,
) -> i32 {
    let paths = derive_auth_paths(base_dir, dev_url);
    if let Some(runtime) = read_live_server_runtime_state(&paths.server_runtime_state_path)
        && let Some(code) = try_live_auth_delete(
            &paths,
            &runtime.origin,
            &format!("/api/auth/sessions/{session_id}"),
            &format!("Revoked session {session_id}.\n"),
            &format!("No active session found for {session_id}.\n"),
        )
    {
        return code;
    }

    let Ok(store) = open_auth_store(&paths) else {
        return 1;
    };
    match revoke_session(store.connection(), session_id) {
        Ok(true) => write_stdout_line(&format!("Revoked session {session_id}.\n"), "auth"),
        Ok(false) => write_stdout_line(
            &format!("No active session found for {session_id}.\n"),
            "auth",
        ),
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}

pub(crate) fn emit_formatted(rendered: &str, raw: bool) -> i32 {
    if raw {
        write_stdout_raw(rendered, "auth output")
    } else {
        write_stdout_line(rendered, "auth output")
    }
}
