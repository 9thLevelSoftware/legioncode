//! Transcript cloning and UUID/compaction-chain remapping for [`crate::claude_history::fork_session`].

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write as _;
use std::path::Path;

use serde_json::{Map, Value};
use uuid::Uuid;

use crate::claude_history::{
    ClaudeHistoryError, ClaudeHistoryOptions, ForkSessionResult, entry_bool, entry_str,
    get_session_messages, parse_jsonl_entries, session_jsonl_path, validate_session_id,
};

/// Forks `<dir>/<session_id>.jsonl` into a new UUID session, remapping `uuid`, `parentUuid`,
/// `logicalParentUuid`, and compaction references cleanly (`claudeHistoryWorker.ts:25-31`).
pub fn fork_session(
    session_id: &str,
    options: &ClaudeHistoryOptions,
) -> Result<ForkSessionResult, ClaudeHistoryError> {
    validate_session_id(session_id)?;
    let dir = options
        .dir
        .as_deref()
        .ok_or(ClaudeHistoryError::MissingDirectory)?;
    let source_path = session_jsonl_path(session_id, Some(dir))?;
    let raw = match fs::read_to_string(&source_path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(ClaudeHistoryError::SessionNotFound(session_id.to_owned()));
        }
        Err(err) => return Err(ClaudeHistoryError::Io(err)),
    };

    let parsed: Vec<Map<String, Value>> = parse_jsonl_entries(&raw)
        .into_iter()
        .filter(|entry| !entry_bool(entry, "isSidechain"))
        .collect();
    if parsed.is_empty() {
        return Err(ClaudeHistoryError::NoMessagesToFork(session_id.to_owned()));
    }

    let sliced = if let Some(target_uuid) = options.up_to_message_id.as_deref() {
        let idx = parsed
            .iter()
            .position(|entry| entry_str(entry, "uuid") == Some(target_uuid))
            .ok_or_else(|| ClaudeHistoryError::UpToMessageNotFound {
                session_id: session_id.to_owned(),
                message_id: target_uuid.to_owned(),
            })?;
        &parsed[..=idx]
    } else {
        parsed.as_slice()
    };

    let maps = build_uuid_maps(sliced);
    let kept: Vec<&Map<String, Value>> = sliced
        .iter()
        .filter(|entry| entry_str(entry, "type") != Some("progress"))
        .collect();
    if kept.is_empty() {
        return Err(ClaudeHistoryError::NoMessagesToFork(session_id.to_owned()));
    }

    let forked_session_id = Uuid::new_v4().to_string();
    let out_lines = rewrite_entries(
        &kept,
        session_id,
        &forked_session_id,
        &maps.by_uuid,
        &maps.uuid_map,
    )?;

    let target_path = session_jsonl_path(&forked_session_id, Some(dir))?;
    write_atomic(&target_path, &format!("{}\n", out_lines.join("\n")))?;

    let read_opts = ClaudeHistoryOptions {
        dir: Some(dir.to_path_buf()),
        include_system_messages: false,
        up_to_message_id: None,
        limit: None,
        offset: 0,
    };
    let forked_messages = get_session_messages(&forked_session_id, &read_opts)?;
    let head_message_id = forked_messages.last().map(|msg| msg.uuid.clone());

    Ok(ForkSessionResult {
        session_id: forked_session_id,
        head_message_id,
    })
}

struct UuidMaps<'a> {
    uuid_map: HashMap<String, String>,
    by_uuid: HashMap<String, &'a Map<String, Value>>,
}

fn build_uuid_maps(sliced: &[Map<String, Value>]) -> UuidMaps<'_> {
    let mut uuid_map = HashMap::new();
    let mut by_uuid = HashMap::new();
    for entry in sliced {
        if let Some(old_uuid) = entry_str(entry, "uuid") {
            uuid_map
                .entry(old_uuid.to_owned())
                .or_insert_with(|| Uuid::new_v4().to_string());
            by_uuid.insert(old_uuid.to_owned(), entry);
        }
    }
    UuidMaps { uuid_map, by_uuid }
}

fn rewrite_entries(
    kept: &[&Map<String, Value>],
    source_session_id: &str,
    forked_session_id: &str,
    by_uuid: &HashMap<String, &Map<String, Value>>,
    uuid_map: &HashMap<String, String>,
) -> Result<Vec<String>, ClaudeHistoryError> {
    let mut out_lines = Vec::with_capacity(kept.len());
    for entry in kept {
        let mut cloned = (*entry).clone();
        let old_uuid = entry_str(entry, "uuid").unwrap_or("");
        if let Some(new_uuid) = uuid_map.get(old_uuid) {
            cloned.insert("uuid".to_owned(), Value::String(new_uuid.clone()));
        }
        let parent = resolve_forked_parent(entry, by_uuid, uuid_map);
        cloned.insert(
            "parentUuid".to_owned(),
            parent.map_or(Value::Null, Value::String),
        );

        if let Some(logical) = entry_str(entry, "logicalParentUuid") {
            let mapped = uuid_map.get(logical).cloned();
            cloned.insert(
                "logicalParentUuid".to_owned(),
                mapped.map_or(Value::Null, Value::String),
            );
        }
        remap_compact_metadata(&mut cloned, uuid_map);
        cloned.insert(
            "sessionId".to_owned(),
            Value::String(forked_session_id.to_owned()),
        );
        cloned.insert("isSidechain".to_owned(), Value::Bool(false));

        let mut forked_from = Map::new();
        forked_from.insert(
            "sessionId".to_owned(),
            Value::String(source_session_id.to_owned()),
        );
        forked_from.insert("messageUuid".to_owned(), Value::String(old_uuid.to_owned()));
        cloned.insert("forkedFrom".to_owned(), Value::Object(forked_from));

        if entry_str(entry, "type") == Some("system")
            && entry_str(entry, "subtype") == Some("model_refusal_fallback")
        {
            cloned.insert("neutralizedByFork".to_owned(), Value::Bool(true));
        }
        out_lines.push(serde_json::to_string(&Value::Object(cloned))?);
    }
    Ok(out_lines)
}

fn resolve_forked_parent(
    entry: &Map<String, Value>,
    by_uuid: &HashMap<String, &Map<String, Value>>,
    uuid_map: &HashMap<String, String>,
) -> Option<String> {
    let mut parent = entry_str(entry, "parentUuid");
    let mut visited = HashSet::new();
    while let Some(pid) = parent {
        let target = by_uuid.get(pid).copied()?;
        if entry_str(target, "type") != Some("progress") {
            return uuid_map.get(pid).cloned();
        }
        if !visited.insert(pid.to_owned()) {
            return uuid_map.get(pid).cloned();
        }
        parent = entry_str(target, "parentUuid");
    }
    None
}

fn remap_compact_metadata(entry: &mut Map<String, Value>, uuid_map: &HashMap<String, String>) {
    let Some(Value::Object(meta)) = entry.get_mut("compactMetadata") else {
        return;
    };
    if let Some(Value::Object(seg)) = meta.get_mut("preservedSegment") {
        for field in ["headUuid", "anchorUuid", "tailUuid"] {
            remap_string_field(seg, field, uuid_map);
        }
    }
    if let Some(Value::Object(msgs)) = meta.get_mut("preservedMessages") {
        remap_string_field(msgs, "anchorUuid", uuid_map);
        for field in ["uuids", "allUuids"] {
            if let Some(Value::Array(arr)) = msgs.get_mut(field) {
                for item in arr.iter_mut() {
                    if let Some(old) = item.as_str()
                        && let Some(new_id) = uuid_map.get(old)
                    {
                        *item = Value::String(new_id.clone());
                    }
                }
            }
        }
    }
}

fn remap_string_field(
    obj: &mut Map<String, Value>,
    field: &str,
    uuid_map: &HashMap<String, String>,
) {
    if let Some(old) = obj.get(field).and_then(Value::as_str)
        && let Some(new_id) = uuid_map.get(old)
    {
        obj.insert(field.to_owned(), Value::String(new_id.clone()));
    }
}

fn write_atomic(path: &Path, content: &str) -> Result<(), ClaudeHistoryError> {
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("fork.jsonl");
    let tmp = path.with_file_name(format!(".{file_name}.{}.tmp", Uuid::new_v4()));
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(content.as_bytes())?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}
