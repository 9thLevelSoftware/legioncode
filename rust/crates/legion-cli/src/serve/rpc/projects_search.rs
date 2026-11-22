//! Workspace entry fuzzy/substring search and content search (`searchEntries`, `searchContents`).

use std::fs;
use std::path::Path;

use legion_protocol::rpc_codec::RpcExit;
use serde::Deserialize;
use serde_json::{Value, json};

use super::workspace_walk::collect_workspace_entries;
use super::{parse_payload, rpc_fail_obj};

const MAX_SEARCH_CONTENT_FILE_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchEntriesPayload {
    cwd: String,
    query: String,
    limit: Option<usize>,
}

pub(crate) async fn search_entries(payload: Value) -> RpcExit {
    let req: SearchEntriesPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let root = Path::new(&req.cwd);
    if !root.is_dir() {
        return rpc_fail_obj(json!({
            "_tag": "WorkspaceRootNotExistsError",
            "workspaceRoot": req.cwd,
            "normalizedWorkspaceRoot": req.cwd,
            "message": format!("Workspace root does not exist: {}", req.cwd),
        }));
    }
    let limit = req.limit.unwrap_or(100).clamp(1, 500);
    let needle = req.query.trim().to_ascii_lowercase();
    let (all_entries, _, skipped) = collect_workspace_entries(root).await;
    let mut matched = Vec::new();
    let mut truncated = false;
    for entry in all_entries {
        let path_str = entry.get("path").and_then(Value::as_str).unwrap_or("");
        if needle.is_empty() || path_str.to_ascii_lowercase().contains(&needle) {
            if matched.len() >= limit {
                truncated = true;
                break;
            }
            matched.push(entry);
        }
    }
    RpcExit::Success {
        value: json!({
            "entries": matched,
            "truncated": truncated,
            "skippedEntryCount": skipped,
        }),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchContentsPayload {
    cwd: String,
    query: String,
    #[serde(default)]
    case_sensitive: Option<bool>,
    #[serde(default)]
    whole_word: Option<bool>,
    #[serde(default)]
    regex: Option<bool>,
    limit: Option<usize>,
}

pub(crate) async fn search_contents(payload: Value) -> RpcExit {
    let req: SearchContentsPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let root = Path::new(&req.cwd);
    if !root.is_dir() {
        return RpcExit::Success {
            value: json!({ "matches": [], "truncated": false, "skippedFileCount": 0 }),
        };
    }
    let matcher = match ContentMatcher::compile(
        &req.query,
        req.case_sensitive.unwrap_or(false),
        req.whole_word.unwrap_or(false),
        req.regex.unwrap_or(false),
    ) {
        Ok(m) => m,
        Err(msg) => {
            return RpcExit::Success {
                value: json!({
                    "matches": [],
                    "truncated": false,
                    "skippedFileCount": 0,
                    "error": { "code": "invalid-regex", "message": msg },
                }),
            };
        }
    };
    let limit = req.limit.unwrap_or(200).clamp(1, 2000);
    let (entries, _, _) = collect_workspace_entries(root).await;
    let mut hit_list = Vec::new();
    let mut truncated = false;
    let mut skipped_files = 0_usize;

    for entry in entries {
        if entry.get("kind").and_then(Value::as_str) != Some("file") {
            continue;
        }
        let Some(rel) = entry.get("path").and_then(Value::as_str) else {
            continue;
        };
        let full = root.join(rel);
        let Ok(meta) = fs::metadata(&full) else {
            skipped_files += 1;
            continue;
        };
        if meta.len() > MAX_SEARCH_CONTENT_FILE_BYTES {
            skipped_files += 1;
            continue;
        }
        let Ok(bytes) = fs::read(&full) else {
            skipped_files += 1;
            continue;
        };
        if bytes.iter().take(4096).any(|&b| b == 0) {
            skipped_files += 1;
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        for (idx, line) in text.lines().enumerate() {
            if let Some(byte_start) = matcher.find_in_line(line) {
                let col_utf16 = line[..byte_start].encode_utf16().count() + 1;
                let preview: String = line.chars().take(400).collect();
                hit_list.push(json!({
                    "path": rel,
                    "line": idx + 1,
                    "column": col_utf16,
                    "lineText": preview,
                }));
                if hit_list.len() >= limit {
                    truncated = true;
                    break;
                }
            }
        }
        if truncated {
            break;
        }
    }

    RpcExit::Success {
        value: json!({
            "matches": hit_list,
            "truncated": truncated,
            "skippedFileCount": skipped_files,
        }),
    }
}

struct ContentMatcher {
    branches: Vec<String>,
    case_sensitive: bool,
    whole_word: bool,
}

impl ContentMatcher {
    fn compile(
        query: &str,
        case_sensitive: bool,
        whole_word: bool,
        is_regex: bool,
    ) -> Result<Self, String> {
        if query.is_empty() {
            return Err("Search query cannot be empty.".to_owned());
        }
        if is_regex && query.ends_with('\\') {
            return Err("Trailing backslash in pattern.".to_owned());
        }
        let raw_branches: Vec<String> = if is_regex {
            let trimmed = query
                .strip_prefix('^')
                .unwrap_or(query)
                .strip_suffix('$')
                .unwrap_or(query);
            trimmed.split('|').map(|s| s.replace("\\b", "")).collect()
        } else {
            vec![query.to_owned()]
        };
        let branches = raw_branches
            .into_iter()
            .filter(|b| !b.is_empty())
            .map(|b| {
                if case_sensitive {
                    b
                } else {
                    b.to_ascii_lowercase()
                }
            })
            .collect();
        Ok(Self {
            branches,
            case_sensitive,
            whole_word,
        })
    }

    fn find_in_line(&self, line: &str) -> Option<usize> {
        let hay_owned;
        let hay = if self.case_sensitive {
            line
        } else {
            hay_owned = line.to_ascii_lowercase();
            &hay_owned
        };
        for branch in &self.branches {
            let mut offset = 0_usize;
            while let Some(pos) = hay[offset..].find(branch) {
                let abs_start = offset + pos;
                let abs_end = abs_start + branch.len();
                if !self.whole_word || is_whole_word(line, abs_start, abs_end) {
                    return Some(abs_start);
                }
                let next_step = line[abs_start..].chars().next().map_or(1, char::len_utf8);
                offset = abs_start + next_step;
            }
        }
        None
    }
}

fn is_whole_word(line: &str, start: usize, end: usize) -> bool {
    let before_ok = line[..start]
        .chars()
        .next_back()
        .is_none_or(|c| !is_word_char(c));
    let after_ok = line[end..].chars().next().is_none_or(|c| !is_word_char(c));
    before_ok && after_ok
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}
