//! Hidden internal worker commands: `__document-text`, `__managed-chatgpt-token`,
//! and `__claude-history`.

use std::path::Path;
use std::time::Duration;

use legion_persistence::claude_history::{
    ClaudeHistoryOptions, fork_session, get_session_messages,
};
use legion_persistence::doc_extractor::{DocExtractError, extract_document_text};
use serde::{Deserialize, Serialize};

use crate::fallback::{
    delegate_to_node, unit_forced_original, write_stdout_line, write_stdout_raw,
};
use crate::pipe_ipc::exchange_json_line;

/// Unit identifier for `RUST_ANYTHING_UNIT_DOCUMENT_TEXT` and fallback logs.
pub const UNIT_DOCUMENT_TEXT: &str = "DOCUMENT_TEXT";
/// Unit identifier for `RUST_ANYTHING_UNIT_MANAGED_TOKEN` and fallback logs.
pub const UNIT_MANAGED_TOKEN: &str = "MANAGED_TOKEN";
/// Unit identifier for `RUST_ANYTHING_UNIT_CLAUDE_HISTORY` and fallback logs.
pub const UNIT_CLAUDE_HISTORY: &str = "CLAUDE_HISTORY";

const MANAGED_TOKEN_REQUEST_BYTES: usize = 1024;
const MANAGED_TOKEN_RESPONSE_BYTES: usize = 64 * 1024;
const MANAGED_TOKEN_TIMEOUT: Duration = Duration::from_secs(5);
const MANAGED_TOKEN_ERROR_MSG: &str =
    "Could not access the managed ChatGPT connection. Reconnect in LegionCode.";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ManagedTokenRequest<'a> {
    version: u32,
    instance_id: &'a str,
    environment_id: &'a str,
    profile_key: &'a str,
}

#[derive(Deserialize)]
struct ManagedTokenResponse {
    token: String,
}

/// Runs `__document-text <path> <ext>` (`binMain.ts:11-21`).
pub fn run_document_text(args: &[String]) -> i32 {
    if unit_forced_original(UNIT_DOCUMENT_TEXT) {
        return delegate_to_node(UNIT_DOCUMENT_TEXT, args);
    }

    let (Some(path), Some(extension)) = (args.get(1), args.get(2)) else {
        eprintln!("Usage: legioncode __document-text <path> <extension>");
        return 1;
    };

    match extract_document_text(Path::new(path), extension) {
        Ok(text) => write_stdout_raw(&text, "extracted document text"),
        Err(err @ DocExtractError::Io(_)) => {
            eprintln!("{err}");
            err.exit_code()
        }
        Err(err) => err.exit_code(),
    }
}

/// Runs `__managed-chatgpt-token <endpoint> <instanceId> <environmentId> <profileKey>`
/// (`cli/managedChatGptToken.ts`).
pub fn run_managed_chatgpt_token(args: &[String]) -> i32 {
    if unit_forced_original(UNIT_MANAGED_TOKEN) {
        return delegate_to_node(UNIT_MANAGED_TOKEN, args);
    }

    let Some([_, endpoint, instance_id, environment_id, profile_key]) = args.get(..5) else {
        eprintln!("{MANAGED_TOKEN_ERROR_MSG}");
        return 1;
    };
    if args.len() != 5
        || endpoint.trim().is_empty()
        || instance_id.is_empty()
        || environment_id.is_empty()
        || !is_sha256_hex(profile_key)
    {
        eprintln!("{MANAGED_TOKEN_ERROR_MSG}");
        return 1;
    }

    let Ok(payload) = serde_json::to_string(&ManagedTokenRequest {
        version: 1,
        instance_id,
        environment_id,
        profile_key,
    }) else {
        eprintln!("{MANAGED_TOKEN_ERROR_MSG}");
        return 1;
    };

    let Ok(reply_line) = exchange_json_line(
        endpoint,
        &payload,
        MANAGED_TOKEN_REQUEST_BYTES,
        MANAGED_TOKEN_RESPONSE_BYTES,
        MANAGED_TOKEN_TIMEOUT,
    ) else {
        eprintln!("{MANAGED_TOKEN_ERROR_MSG}");
        return 1;
    };

    let Ok(decoded) = serde_json::from_str::<ManagedTokenResponse>(&reply_line) else {
        eprintln!("{MANAGED_TOKEN_ERROR_MSG}");
        return 1;
    };
    if decoded.token.is_empty() {
        eprintln!("{MANAGED_TOKEN_ERROR_MSG}");
        return 1;
    }

    write_stdout_raw(&decoded.token, "managed ChatGPT token")
}

/// Runs `__claude-history <method> <sessionId> [optionsJson]` (`claudeHistoryWorker.ts`).
pub fn run_claude_history(args: &[String]) -> i32 {
    if unit_forced_original(UNIT_CLAUDE_HISTORY) {
        return delegate_to_node(UNIT_CLAUDE_HISTORY, args);
    }

    let (Some(method), Some(session_id)) = (args.get(1).map(String::as_str), args.get(2)) else {
        eprintln!("Usage: legioncode __claude-history <method> <sessionId> [optionsJson]");
        return 1;
    };

    let options: ClaudeHistoryOptions = match args.get(3) {
        Some(raw) if !raw.trim().is_empty() => match serde_json::from_str(raw) {
            Ok(parsed) => parsed,
            Err(err) => {
                eprintln!("Invalid Claude history options JSON: {err}");
                return 1;
            }
        },
        _ => ClaudeHistoryOptions::default(),
    };

    let json = match method {
        "getSessionMessages" => match get_session_messages(session_id, &options) {
            Ok(messages) => serde_json::to_string(&messages),
            Err(err) => {
                eprintln!("{err}");
                return 1;
            }
        },
        "forkSession" => match fork_session(session_id, &options) {
            Ok(result) => serde_json::to_string(&result),
            Err(err) => {
                eprintln!("{err}");
                return 1;
            }
        },
        other => {
            eprintln!("Unknown Claude history worker method: {other}");
            return 1;
        }
    };

    match json {
        Ok(encoded) => write_stdout_line(&encoded, "claude history output"),
        Err(err) => {
            eprintln!("Failed to serialize Claude history output: {err}");
            1
        }
    }
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}
