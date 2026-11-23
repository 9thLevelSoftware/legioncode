#![forbid(unsafe_code)]
//! Feedback trace builder and byte/entry budget trimmer.

use std::collections::{HashMap, HashSet};

use crate::scrub::scrub_feedback_text;
use crate::time::parse_iso_millis;
use crate::trace_types::{
    BuildFeedbackTraceInput, FEEDBACK_TRACE_MAX_BYTES, FeedbackTrace, FeedbackTraceEntry,
    FeedbackTraceEntryKind, FeedbackTraceOutcome, MAX_TRACE_ENTRIES, MAX_TRACE_TEXT_CHARS,
    OTHER_TOOL, TraceItemInput, TraceItemKind, TraceItemStatus, TraceRunInput,
    feedback_trace_window, resolve_feedback_tool_name,
};

fn is_valid_builtin_tool_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphabetic() {
        return false;
    }
    name.len() <= 64 && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn format_tool_name(
    item: &TraceItemInput,
    registered_tools: &[&str],
    names_built_ins: bool,
) -> String {
    if item.kind == TraceItemKind::CommandExecution {
        return "shell".to_owned();
    }
    if item.kind != TraceItemKind::DynamicTool {
        return match item.kind {
            TraceItemKind::UserMessage => "user_message",
            TraceItemKind::AssistantMessage => "assistant_message",
            TraceItemKind::FileChange => "file_change",
            TraceItemKind::FileSearch => "file_search",
            TraceItemKind::WebSearch => "web_search",
            TraceItemKind::Subagent => "subagent",
            TraceItemKind::CommandExecution | TraceItemKind::DynamicTool => unreachable!(),
        }
        .to_owned();
    }
    let raw = item.tool_name.as_deref().unwrap_or("");
    if let Some(resolved) = resolve_feedback_tool_name(raw, registered_tools) {
        return resolved;
    }
    if names_built_ins
        && !item.tool_is_integration
        && !(raw.len() >= 3 && raw[..3].eq_ignore_ascii_case("mcp"))
        && is_valid_builtin_tool_name(raw)
    {
        raw.to_owned()
    } else {
        OTHER_TOOL.to_owned()
    }
}

fn item_outcome(item: &TraceItemInput) -> FeedbackTraceOutcome {
    if item.status == TraceItemStatus::Failed {
        return FeedbackTraceOutcome::Error;
    }
    if item.status != TraceItemStatus::Completed {
        return FeedbackTraceOutcome::Unfinished;
    }
    if item.kind == TraceItemKind::CommandExecution
        && (item.exit_code.unwrap_or(0) != 0 || item.output_indicates_failure)
    {
        FeedbackTraceOutcome::Error
    } else {
        FeedbackTraceOutcome::Ok
    }
}

/// Builds a redacted outline or conversation [`FeedbackTrace`] capped at `80` entries and `16,000` JSON bytes.
pub fn build_feedback_trace(input: &BuildFeedbackTraceInput<'_>) -> FeedbackTrace {
    let window = feedback_trace_window(input.turns);
    let mut started: Vec<&TraceRunInput> = input
        .runs
        .iter()
        .filter(|r| r.started_at.is_some())
        .collect();
    started.sort_by_key(|r| r.ordinal);

    let total_started = started.len();
    let keep_start = total_started.saturating_sub(window);
    let runs = &started[keep_start..];
    let first_text_turn = if input.with_text {
        runs.len().saturating_sub(input.turns) + 1
    } else {
        usize::MAX
    };

    let mut turn_of: HashMap<&str, (usize, &TraceRunInput)> = HashMap::new();
    for (idx, run) in runs.iter().enumerate() {
        turn_of.insert(run.id.as_str(), (idx + 1, *run));
    }

    let mut matched: Vec<(usize, &TraceRunInput, &TraceItemInput)> = input
        .items
        .iter()
        .filter(|item| item.parent_item_id.is_none() && !is_submitting_call(item, input))
        .filter_map(|item| {
            let rid = item.run_id.as_deref()?;
            let &(turn, run) = turn_of.get(rid)?;
            Some((turn, run, item))
        })
        .collect();
    matched.sort_by_key(|(turn, _, item)| (*turn, item.ordinal));

    let mut truncated = input.turns > window && total_started >= window;
    let entries: Vec<FeedbackTraceEntry> = matched
        .into_iter()
        .map(|(turn, run, item)| {
            map_trace_entry(turn, run, item, first_text_turn, input, &mut truncated)
        })
        .collect();

    trim_trace_to_budget(runs.len(), entries, truncated)
}

fn is_submitting_call(item: &TraceItemInput, input: &BuildFeedbackTraceInput<'_>) -> bool {
    let Some(ref rid) = item.run_id else {
        return false;
    };
    if rid != input.submitting_run_id
        || item.kind != TraceItemKind::DynamicTool
        || item_outcome(item) != FeedbackTraceOutcome::Unfinished
    {
        return false;
    }
    let raw = item.tool_name.as_deref().unwrap_or("");
    matches!(
        resolve_feedback_tool_name(raw, input.registered_tools).as_deref(),
        Some("legion_feedback" | "t3_feedback" | "feedback")
    )
}

fn map_trace_entry(
    turn: usize,
    run: &TraceRunInput,
    item: &TraceItemInput,
    first_text_turn: usize,
    input: &BuildFeedbackTraceInput<'_>,
    truncated: &mut bool,
) -> FeedbackTraceEntry {
    match item.kind {
        TraceItemKind::UserMessage | TraceItemKind::AssistantMessage => {
            let raw_text = item.text.as_deref().unwrap_or("");
            let length = raw_text.chars().count();
            let kind = if item.kind == TraceItemKind::UserMessage {
                FeedbackTraceEntryKind::User
            } else {
                FeedbackTraceEntryKind::Assistant
            };
            let text = if turn >= first_text_turn {
                let scrubbed = scrub_feedback_text(raw_text, input.sensitive, MAX_TRACE_TEXT_CHARS);
                if scrubbed.cut {
                    *truncated = true;
                }
                Some(scrubbed.text)
            } else {
                None
            };
            FeedbackTraceEntry {
                turn,
                kind,
                tool: None,
                outcome: None,
                duration_ms: None,
                length: Some(length),
                text,
            }
        }
        _ => {
            let names_built_ins = matches!(run.driver.as_deref(), Some("claudeAgent" | "codex"));
            let duration_ms = match (
                item.started_at.as_deref().and_then(parse_iso_millis),
                item.completed_at.as_deref().and_then(parse_iso_millis),
            ) {
                (Some(s), Some(e)) => Some(e.saturating_sub(s).max(0)),
                _ => None,
            };
            FeedbackTraceEntry {
                turn,
                kind: FeedbackTraceEntryKind::Tool,
                tool: Some(format_tool_name(
                    item,
                    input.registered_tools,
                    names_built_ins,
                )),
                outcome: Some(item_outcome(item)),
                duration_ms,
                length: None,
                text: None,
            }
        }
    }
}

fn trim_trace_to_budget(
    turns: usize,
    mut entries: Vec<FeedbackTraceEntry>,
    mut truncated: bool,
) -> FeedbackTrace {
    while entries.len() > MAX_TRACE_ENTRIES {
        let drop_idx = entries
            .iter()
            .position(|e| e.kind == FeedbackTraceEntryKind::Tool)
            .unwrap_or(0);
        entries.remove(drop_idx);
        truncated = true;
    }

    loop {
        let text_turns = entries
            .iter()
            .filter(|e| e.text.is_some())
            .map(|e| e.turn)
            .collect::<HashSet<_>>()
            .len();
        let candidate = FeedbackTrace {
            turns,
            text_turns,
            truncated,
            entries: entries.clone(),
        };
        let encoded_len = serde_json::to_vec(&candidate).map_or(0, |v| v.len());
        if encoded_len <= FEEDBACK_TRACE_MAX_BYTES {
            return candidate;
        }
        truncated = true;
        if let Some(entry) = entries.iter_mut().find(|e| e.text.is_some()) {
            entry.text = None;
        } else if !entries.is_empty() {
            entries.remove(0);
        } else {
            return candidate;
        }
    }
}
