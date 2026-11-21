//! Pure Safe Rust reader and session forker for Claude Code `.jsonl` transcripts.
//!
//! Mirrors `apps/server/src/claudeHistoryWorker.ts` (`getSessionMessages` + `forkSession` with
//! system-message filtering, `upToMessageId` slicing, compaction link remapping, and `headMessageId`).

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;

pub use crate::claude_history_fork::fork_session;

/// Errors returned by [`get_session_messages`] and [`fork_session`].
#[derive(Debug, Error)]
pub enum ClaudeHistoryError {
    /// Filesystem I/O error reading or writing transcript JSONL files.
    #[error("Claude history I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Invalid or empty session identifier.
    #[error("invalid Claude session ID '{0}'")]
    InvalidSessionId(String),
    /// Missing directory option when resolving `<dir>/<session_id>.jsonl`.
    #[error("Claude history directory option is required")]
    MissingDirectory,
    /// Target session transcript does not exist on disk.
    #[error("Claude session '{0}' not found")]
    SessionNotFound(String),
    /// Source session contained no non-sidechain messages eligible for forking.
    #[error("Claude session '{0}' has no messages to fork")]
    NoMessagesToFork(String),
    /// Requested `upToMessageId` anchor was absent from the session transcript.
    #[error("message '{message_id}' not found in session '{session_id}'")]
    UpToMessageNotFound {
        /// Session identifier searched.
        session_id: String,
        /// Missing message UUID.
        message_id: String,
    },
    /// JSON encoding error.
    #[error("Claude history JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Options accepted by [`get_session_messages`] and [`fork_session`], matching
/// `claudeHistoryWorker.ts`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ClaudeHistoryOptions {
    /// Directory containing `<session_id>.jsonl`.
    pub dir: Option<PathBuf>,
    /// Include `type: "system"` entries when returning messages.
    pub include_system_messages: bool,
    /// Slice forked transcript up to and including this message UUID.
    pub up_to_message_id: Option<String>,
    /// Maximum number of messages returned by [`get_session_messages`].
    pub limit: Option<usize>,
    /// Number of leading messages skipped by [`get_session_messages`].
    pub offset: usize,
}

/// One historical transcript message returned by [`get_session_messages`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionMessage {
    /// Transcript message role/kind (`"user"`, `"assistant"`, or `"system"`).
    #[serde(rename = "type")]
    pub kind: String,
    /// Unique message UUID.
    pub uuid: String,
    /// Owning session identifier.
    pub session_id: String,
    /// Raw Anthropic SDK `message` payload.
    pub message: Value,
    /// Optional parent tool use ID (`null` for top-level transcripts).
    pub parent_tool_use_id: Option<String>,
    /// Optional parent subagent ID (`null` for top-level transcripts).
    pub parent_agent_id: Option<String>,
    /// Optional ISO-8601 timestamp string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
}

/// Result returned by [`fork_session`], matching `claudeHistoryWorker.ts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForkSessionResult {
    /// UUID of the newly created forked session.
    pub session_id: String,
    /// UUID of the final message in the forked session (when non-empty).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head_message_id: Option<String>,
}

/// Reads and filters chronological conversation messages from `<dir>/<session_id>.jsonl`.
pub fn get_session_messages(
    session_id: &str,
    options: &ClaudeHistoryOptions,
) -> Result<Vec<SessionMessage>, ClaudeHistoryError> {
    validate_session_id(session_id)?;
    let path = session_jsonl_path(session_id, options.dir.as_deref())?;
    let raw = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(ClaudeHistoryError::Io(err)),
    };

    let entries = parse_jsonl_entries(&raw);
    let ordered = build_conversation_chain(&entries);
    let mut messages = Vec::new();

    for entry in ordered {
        let Some(kind) = entry_str(&entry, "type") else {
            continue;
        };
        let keep_kind = kind == "user"
            || kind == "assistant"
            || (kind == "system" && options.include_system_messages);
        if !keep_kind
            || entry_bool(&entry, "isSidechain")
            || entry_bool(&entry, "isMeta")
            || entry.get("teamName").is_some_and(|v| !v.is_null())
        {
            continue;
        }
        let Some(uuid) = entry_str(&entry, "uuid") else {
            continue;
        };
        let entry_session_id = entry_str(&entry, "sessionId").unwrap_or(session_id);
        let msg_payload = entry.get("message").cloned().unwrap_or(Value::Null);
        let timestamp = entry_str(&entry, "timestamp").map(str::to_owned);
        messages.push(SessionMessage {
            kind: kind.to_owned(),
            uuid: uuid.to_owned(),
            session_id: entry_session_id.to_owned(),
            message: msg_payload,
            parent_tool_use_id: None,
            parent_agent_id: None,
            timestamp,
        });
    }

    Ok(slice_offset_limit(messages, options.offset, options.limit))
}

pub(crate) fn validate_session_id(session_id: &str) -> Result<(), ClaudeHistoryError> {
    if session_id.is_empty()
        || session_id.contains('/')
        || session_id.contains('\\')
        || session_id.contains("..")
        || session_id.contains('\0')
    {
        return Err(ClaudeHistoryError::InvalidSessionId(session_id.to_owned()));
    }
    Ok(())
}

pub(crate) fn session_jsonl_path(
    session_id: &str,
    dir: Option<&Path>,
) -> Result<PathBuf, ClaudeHistoryError> {
    let base = dir.ok_or(ClaudeHistoryError::MissingDirectory)?;
    Ok(base.join(format!("{session_id}.jsonl")))
}

pub(crate) fn parse_jsonl_entries(raw: &str) -> Vec<Map<String, Value>> {
    let mut entries = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(trimmed)
            && obj.get("uuid").and_then(Value::as_str).is_some()
        {
            entries.push(obj);
        }
    }
    entries
}

fn build_conversation_chain(entries: &[Map<String, Value>]) -> Vec<Map<String, Value>> {
    if entries.is_empty() {
        return Vec::new();
    }
    let mut by_uuid: HashMap<&str, &Map<String, Value>> = HashMap::with_capacity(entries.len());
    let mut children: HashMap<&str, Vec<&Map<String, Value>>> = HashMap::new();
    for entry in entries {
        if let Some(uuid) = entry_str(entry, "uuid") {
            by_uuid.insert(uuid, entry);
        }
        if let Some(parent) = entry_str(entry, "parentUuid")
            && entry_str(entry, "type") != Some("user")
            && entry_str(entry, "type") != Some("assistant")
        {
            children.entry(parent).or_default().push(entry);
        }
    }

    let Some(leaf) = entries.iter().rev().find(|entry| {
        matches!(entry_str(entry, "type"), Some("user" | "assistant"))
            && !entry_bool(entry, "isSidechain")
    }) else {
        return Vec::new();
    };

    let mut chain = Vec::new();
    let mut seen = HashSet::new();
    let mut cursor = Some(leaf);
    while let Some(node) = cursor {
        let Some(uuid) = entry_str(node, "uuid") else {
            break;
        };
        if !seen.insert(uuid) {
            break;
        }
        chain.push((*node).clone());
        let parent_id =
            entry_str(node, "parentUuid").or_else(|| entry_str(node, "logicalParentUuid"));
        cursor = parent_id.and_then(|pid| by_uuid.get(pid).copied());
    }
    chain.reverse();

    if let Some(leaf_uuid) = entry_str(leaf, "uuid")
        && let Some(trailing) = children.get(leaf_uuid)
    {
        for node in trailing {
            if let Some(uuid) = entry_str(node, "uuid")
                && seen.insert(uuid)
            {
                chain.push((*node).clone());
            }
        }
    }
    chain
}

pub(crate) fn entry_str<'a>(entry: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    entry.get(key).and_then(Value::as_str)
}

pub(crate) fn entry_bool(entry: &Map<String, Value>, key: &str) -> bool {
    entry.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn slice_offset_limit(
    messages: Vec<SessionMessage>,
    offset: usize,
    limit: Option<usize>,
) -> Vec<SessionMessage> {
    if offset >= messages.len() {
        return Vec::new();
    }
    let iter = messages.into_iter().skip(offset);
    match limit {
        Some(n) if n > 0 => iter.take(n).collect(),
        _ => iter.collect(),
    }
}
