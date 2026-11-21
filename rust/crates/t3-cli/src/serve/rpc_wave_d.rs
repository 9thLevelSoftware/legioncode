//! Wave D WS RPC handlers (`providers.listDrivers`, `checkpoints.getDiff`,
//! `diagnostics.traceSummary`, `themes.read`) wired into `t3 serve /ws`.

use std::env;
use std::path::PathBuf;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Value, json};
use t3_contracts::ids::ThreadId;
use t3_persistence::sqlite::SqliteStore;
use t3_protocol::rpc_codec::RpcExit;
use t3_providers::drivers::{BUILTIN_DRIVER_SPECS, ProviderDriverSpec};
use t3_remote::theme::{
    BUILT_IN_THEME_IDS, is_valid_theme_id, read_default_theme, read_published_themes,
};
use t3_remote::trace::{
    DEFAULT_TRACE_MAX_FILES, format_empty_trace_message, format_trace_summary,
    summarize_trace_files,
};
use t3_scm::checkpoints::checkpoint_ref_for_turn;
use t3_scm::checkpoints::diff_query::{
    CheckpointDiffQuery, DEFAULT_MAX_OUTPUT_BYTES, DiffFileInput, DiffQueryOptions,
};
use t3_scm::checkpoints::store::CheckpointStore;

use super::AppState;
use super::ws_mcp::rpc_fail;
use crate::cmd_trace::{DEFAULT_TRACE_LIMIT, current_epoch_ms, parse_duration_ms};

/// Dispatches Wave D WS RPC methods (`providers.listDrivers`, `checkpoints.getDiff`,
/// `diagnostics.traceSummary`, `themes.read`).
pub(crate) fn dispatch(state: &Arc<AppState>, tag: &str, payload: Value) -> RpcExit {
    match tag {
        "providers.listDrivers" => handle_list_drivers(payload),
        "checkpoints.getDiff" => handle_get_diff(state, payload),
        "diagnostics.traceSummary" => handle_trace_summary(state, payload),
        "themes.read" => handle_themes_read(state, payload),
        _ => rpc_fail(
            "UnsupportedRpcMethod",
            &format!("Unsupported RPC method `{tag}`"),
        ),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ListDriversPayload {
    acp_only: Option<bool>,
    text_generation_only: Option<bool>,
    slug: Option<String>,
}

fn handle_list_drivers(payload: Value) -> RpcExit {
    let filter: ListDriversPayload = if payload.is_null() {
        ListDriversPayload::default()
    } else {
        match serde_json::from_value(payload) {
            Ok(parsed) => parsed,
            Err(err) => return rpc_fail("ParseError", &err.to_string()),
        }
    };

    let drivers: Vec<ProviderDriverSpec> = BUILTIN_DRIVER_SPECS
        .iter()
        .copied()
        .filter(|spec| {
            filter
                .acp_only
                .is_none_or(|only| !only || spec.supports_acp)
        })
        .filter(|spec| {
            filter
                .text_generation_only
                .is_none_or(|only| !only || spec.supports_text_generation)
        })
        .filter(|spec| filter.slug.as_deref().is_none_or(|slug| spec.slug == slug))
        .collect();

    RpcExit::Success {
        value: json!({ "drivers": drivers }),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InlineDiffFile {
    old_path: String,
    new_path: String,
    old_content: String,
    new_content: String,
    #[serde(default)]
    is_binary: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct GetDiffPayload {
    thread_id: Option<String>,
    from_turn_count: Option<u32>,
    to_turn_count: Option<u32>,
    context_lines: Option<usize>,
    ignore_whitespace: Option<bool>,
    max_output_bytes: Option<usize>,
    files: Option<Vec<InlineDiffFile>>,
}

fn handle_get_diff(state: &Arc<AppState>, payload: Value) -> RpcExit {
    let req: GetDiffPayload = match serde_json::from_value(payload) {
        Ok(parsed) => parsed,
        Err(err) => return rpc_fail("ParseError", &err.to_string()),
    };

    if let Some(inline_files) = req.files {
        let inputs: Vec<DiffFileInput<'_>> = inline_files
            .iter()
            .map(|f| DiffFileInput {
                old_path: &f.old_path,
                new_path: &f.new_path,
                old_content: &f.old_content,
                new_content: &f.new_content,
                is_binary: f.is_binary,
            })
            .collect();
        let options = DiffQueryOptions {
            context_lines: req.context_lines.unwrap_or(3),
            ignore_whitespace: req.ignore_whitespace.unwrap_or(false),
            max_output_bytes: req.max_output_bytes.unwrap_or(DEFAULT_MAX_OUTPUT_BYTES),
        };
        let computed = CheckpointDiffQuery::compute_files(&inputs, options);
        return RpcExit::Success {
            value: json!({
                "threadId": req.thread_id,
                "fromTurnCount": req.from_turn_count.unwrap_or(0),
                "toTurnCount": req.to_turn_count,
                "diff": computed.diff,
                "truncated": computed.truncated,
                "filesChanged": computed.files_changed,
                "insertions": computed.insertions,
                "deletions": computed.deletions,
            }),
        };
    }

    let Some(raw_thread_id) = req.thread_id.as_deref() else {
        return rpc_fail("InvalidInput", "Missing `threadId` or inline `files`.");
    };
    let thread_id = match ThreadId::new(raw_thread_id) {
        Ok(id) => id,
        Err(err) => return rpc_fail("InvalidInput", &err.to_string()),
    };
    let Some(to_turn) = req.to_turn_count else {
        return rpc_fail("InvalidInput", "Missing `toTurnCount`.");
    };
    let from_turn = req.from_turn_count.unwrap_or(0);
    if from_turn > to_turn {
        return rpc_fail(
            "InvalidInput",
            &format!("fromTurnCount ({from_turn}) exceeds toTurnCount ({to_turn})"),
        );
    }
    if from_turn == to_turn {
        return RpcExit::Success {
            value: json!({
                "threadId": thread_id.as_str(),
                "fromTurnCount": from_turn,
                "toTurnCount": to_turn,
                "diff": "",
                "truncated": false,
            }),
        };
    }

    let store = match SqliteStore::open(&state.paths.db_path) {
        Ok(store) => store,
        Err(err) => return rpc_fail("SqliteError", &err.to_string()),
    };
    let checkpoints = CheckpointStore::new(store.connection());
    match checkpoints.get_diff_blob(&thread_id, from_turn, to_turn) {
        Ok(Some(blob)) => RpcExit::Success {
            value: json!({
                "threadId": blob.thread_id.as_str(),
                "fromTurnCount": blob.from_turn_count,
                "toTurnCount": blob.to_turn_count,
                "diff": blob.diff,
                "createdAt": blob.created_at,
                "truncated": false,
            }),
        },
        Ok(None) => {
            let git_ref = checkpoint_ref_for_turn(&thread_id, to_turn)
                .ok()
                .map(|r| r.as_str().to_owned());
            rpc_fail(
                "CheckpointDiffUnavailable",
                &format!(
                    "No stored checkpoint diff for `{}` turns {from_turn}..{to_turn} (ref {:?}).",
                    thread_id.as_str(),
                    git_ref
                ),
            )
        }
        Err(err) => rpc_fail("CheckpointError", &err.to_string()),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct TraceSummaryPayload {
    since_ms: Option<i64>,
    since: Option<String>,
    max_files: Option<usize>,
    limit: Option<usize>,
}

fn handle_trace_summary(state: &Arc<AppState>, payload: Value) -> RpcExit {
    let req: TraceSummaryPayload = if payload.is_null() {
        TraceSummaryPayload::default()
    } else {
        match serde_json::from_value(payload) {
            Ok(parsed) => parsed,
            Err(err) => return rpc_fail("ParseError", &err.to_string()),
        }
    };

    let since_ms = if let Some(ms) = req.since_ms {
        Some(ms)
    } else if let Some(raw_since) = req.since.as_deref() {
        let Some(dur_ms) = parse_duration_ms(raw_since) else {
            return rpc_fail(
                "InvalidInput",
                "Invalid `since` duration. Use values like 5m, 1h, 30d, or 15 minutes.",
            );
        };
        Some(current_epoch_ms().saturating_sub(dur_ms))
    } else {
        None
    };

    let trace_path = env::var("T3CODE_TRACE_FILE")
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .map_or_else(|| state.paths.server_trace_path.clone(), PathBuf::from);
    let max_files = req.max_files.unwrap_or(DEFAULT_TRACE_MAX_FILES);
    let limit = req.limit.unwrap_or(DEFAULT_TRACE_LIMIT).max(1);

    let summary = match summarize_trace_files(&trace_path, max_files, since_ms) {
        Ok(summary) => summary,
        Err(err) => return rpc_fail("TraceFileReadError", &err.to_string()),
    };

    let formatted = if summary.span_count == 0 {
        format_empty_trace_message(&trace_path, since_ms.is_some(), summary.skipped_line_count)
    } else {
        format_trace_summary(&summary, limit)
    };

    RpcExit::Success {
        value: json!({
            "traceFilePath": trace_path.to_string_lossy(),
            "formatted": formatted,
            "summary": summary,
        }),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ThemesReadPayload {
    theme_id: Option<String>,
}

fn handle_themes_read(state: &Arc<AppState>, payload: Value) -> RpcExit {
    let req: ThemesReadPayload = if payload.is_null() {
        ThemesReadPayload::default()
    } else {
        match serde_json::from_value(payload) {
            Ok(parsed) => parsed,
            Err(err) => return rpc_fail("ParseError", &err.to_string()),
        }
    };

    let current = match read_default_theme(&state.paths.settings_path) {
        Ok(theme) => theme.unwrap_or_default(),
        Err(err) => return rpc_fail("ServerSettingsError", &err.to_string()),
    };

    let published = read_published_themes(&state.paths.environment_themes_dir);
    let published_summaries: Vec<Value> = published
        .iter()
        .map(|entry| {
            json!({
                "id": entry.id,
                "name": entry.theme.name,
                "appearance": entry.theme.appearance,
                "hasCss": entry.theme.css.is_some(),
                "hasColors": entry.theme.has_colors(),
            })
        })
        .collect();

    let requested_theme = if let Some(requested_id) = req
        .theme_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if !is_valid_theme_id(requested_id) {
            return rpc_fail(
                "EnvironmentThemeRefusedError",
                &format!("Theme \"{requested_id}\" is not a valid theme id."),
            );
        }
        let Some(found) = published.iter().find(|entry| entry.id == requested_id) else {
            return rpc_fail(
                "EnvironmentThemeRefusedError",
                &format!("Theme \"{requested_id}\" is not a published theme."),
            );
        };
        Some(json!({
            "id": found.id,
            "theme": found.theme,
        }))
    } else {
        None
    };

    RpcExit::Success {
        value: json!({
            "current": current,
            "builtIn": BUILT_IN_THEME_IDS,
            "themesDir": state.paths.environment_themes_dir.to_string_lossy(),
            "published": published_summaries,
            "theme": requested_theme,
        }),
    }
}
