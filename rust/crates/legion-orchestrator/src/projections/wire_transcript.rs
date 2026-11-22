//! Thread transcript JSON query (`OrchestrationV2ThreadTranscript`) and Markdown renderer.

use std::fmt::Write as _;

use rusqlite::Connection;
use serde_json::Value;

use crate::projections::ProjectionError;
use crate::projections::wire_thread::{
    load_canonical_thread_projection, project_thread_projection_for_wire,
};
use crate::projections::wire_types::{WireProjectedTurnItem, WireThreadTranscript};

/// Queries structured JSON transcript (`OrchestrationV2ThreadTranscript`) matching
/// `apps/server` `/api/orchestration/threads/:threadId/transcript`.
pub fn query_wire_thread_transcript(
    conn: &Connection,
    thread_id: &str,
) -> Result<WireThreadTranscript, ProjectionError> {
    let (_, canonical) = load_canonical_thread_projection(conn, thread_id)?;
    let projected = project_thread_projection_for_wire(canonical);
    let id = projected
        .thread
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or(thread_id)
        .to_owned();
    let title = projected
        .thread
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    Ok(WireThreadTranscript {
        thread_id: id,
        title,
        updated_at: projected.updated_at,
        items: projected.visible_turn_items,
    })
}

/// Renders a readable Markdown transcript of `thread_id` from its visible timeline items.
pub fn render_wire_thread_transcript(
    conn: &Connection,
    thread_id: &str,
) -> Result<String, ProjectionError> {
    let transcript = query_wire_thread_transcript(conn, thread_id)?;
    let mut md = String::new();
    let _ = writeln!(md, "# {}", transcript.title);
    let _ = writeln!(md);
    let _ = writeln!(md, "- Thread ID: `{}`", transcript.thread_id);
    let _ = writeln!(md, "- Updated: {}", transcript.updated_at);

    for row in &transcript.items {
        render_item_section(&mut md, row);
    }
    Ok(md)
}

fn render_item_section(md: &mut String, row: &WireProjectedTurnItem) {
    let item = &row.item;
    let kind = item
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let _ = writeln!(md);
    match kind {
        "user_message" => {
            let text = item.get("text").and_then(Value::as_str).unwrap_or("");
            let _ = writeln!(md, "## User\n\n{text}");
        }
        "assistant_message" => {
            let text = item.get("text").and_then(Value::as_str).unwrap_or("");
            let _ = writeln!(md, "## Assistant\n\n{text}");
        }
        "reasoning" => {
            let text = item
                .get("text")
                .or_else(|| item.get("summary"))
                .and_then(Value::as_str)
                .unwrap_or("");
            if !text.is_empty() {
                let _ = writeln!(md, "### Reasoning\n\n{text}");
            }
        }
        "proposed_plan" => {
            let body = item.get("markdown").and_then(Value::as_str).unwrap_or("");
            let _ = writeln!(md, "### Proposed Plan\n\n{body}");
        }
        "todo_list" => {
            let _ = writeln!(md, "### Todo List\n");
            if let Some(steps) = item.get("steps").and_then(Value::as_array) {
                for step in steps {
                    let done = step.get("status").and_then(Value::as_str) == Some("completed");
                    let mark = if done { "x" } else { " " };
                    let text = step.get("text").and_then(Value::as_str).unwrap_or("");
                    let _ = writeln!(md, "- [{mark}] {text}");
                }
            }
        }
        "command_execution" => {
            let cmd = item.get("command").and_then(Value::as_str).unwrap_or("");
            let status = item.get("status").and_then(Value::as_str).unwrap_or("");
            let _ = writeln!(md, "### Command ({status})\n\n```sh\n{cmd}\n```");
        }
        "file_change" => {
            let path = item.get("path").and_then(Value::as_str).unwrap_or("");
            let op = item.get("kind").and_then(Value::as_str).unwrap_or("change");
            let _ = writeln!(md, "### File Change\n\n- `{path}` ({op})");
        }
        "subagent" => {
            let title = item
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("Subagent");
            let status = item.get("status").and_then(Value::as_str).unwrap_or("");
            let _ = writeln!(md, "### Subagent: {title} ({status})");
            if let Some(res) = item.get("result").and_then(Value::as_str)
                && !res.is_empty()
            {
                let _ = writeln!(md, "\n{res}");
            }
        }
        "error" => {
            let msg = item.get("message").and_then(Value::as_str).unwrap_or("");
            let _ = writeln!(md, "### Error\n\n> {msg}");
        }
        "fork" => {
            let title = item
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("Forked from conversation");
            let _ = writeln!(md, "---\n\n*{title}*");
        }
        _ => {
            let status = item.get("status").and_then(Value::as_str).unwrap_or("");
            let _ = writeln!(md, "### {kind} ({status})");
        }
    }
}
