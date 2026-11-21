//! Deterministic `Replay (dev)` scenario and turn-event generator for zero-cost
//! integration tests without network calls or paid API usage.

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::acp::{
    PromptContentBlock, SessionUpdateKind, SessionUpdateNotification, ToolCallKind,
    ToolCallLocation, ToolCallStatus, ToolCallUpdatePayload,
};

/// Built-in deterministic replay scenarios matching T3 Code's `Replay (dev)` agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DevReplayScenario {
    /// End-to-end showcase mixing narration chunks, file read, edit, and command run.
    Showcase,
    /// Structured todo-list / plan updates followed by completion summary.
    TodoList,
    /// Tool execution requiring an approval prompt before completion.
    Approval,
    /// Interactive clarification question flow.
    Question,
    /// Multi-turn conversation responding distinctly per `turn_index`.
    MultiTurn,
    /// Command execution streaming incremental output chunks.
    CommandOutput,
}

/// Metadata summarizing one deterministic replay script.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DevReplayTurnScript {
    /// Scenario identifier.
    pub scenario: DevReplayScenario,
    /// Short display title for test fixtures.
    pub label: &'static str,
    /// Ordered ACP `session/update` notifications emitted for this turn.
    pub events: Vec<SessionUpdateKind>,
}

/// Splits `text` into deterministic multi-word streaming chunks (`words_per_chunk`
/// words per chunk, preserving single spaces so rejoining reconstructs normalized text).
#[must_use]
pub fn stream_word_chunks(text: &str, words_per_chunk: usize) -> Vec<String> {
    let chunk_words = words_per_chunk.max(1);
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return Vec::new();
    }
    let total_chunks = words.chunks(chunk_words).len();
    words
        .chunks(chunk_words)
        .enumerate()
        .map(|(idx, slice)| {
            let mut joined = slice.join(" ");
            if idx + 1 < total_chunks {
                joined.push(' ');
            }
            joined
        })
        .collect()
}

/// Substitutes `{workspace}`, `{sessionId}`, and `{toolCallId}` placeholders in
/// deterministic replay payloads.
#[must_use]
pub fn materialize_template(
    template: &str,
    workspace: &str,
    session_id: &str,
    tool_call_id: &str,
) -> String {
    template
        .replace("{workspace}", workspace)
        .replace("{sessionId}", session_id)
        .replace("{toolCallId}", tool_call_id)
}

/// Generates a deterministic sequence of ACP [`SessionUpdateNotification`] items
/// for `scenario` and `turn_index` at zero external provider cost.
///
/// # Examples
///
/// ```
/// use t3_providers::acp::SessionUpdateKind;
/// use t3_providers::drivers::{DevReplayScenario, generate_dev_replay_events};
///
/// let a = generate_dev_replay_events("sess-1", "/repo", DevReplayScenario::Showcase, 0);
/// let b = generate_dev_replay_events("sess-1", "/repo", DevReplayScenario::Showcase, 0);
/// assert_eq!(a, b);
/// assert!(a.iter().any(|n| matches!(n.update, SessionUpdateKind::ToolCall(_))));
/// ```
#[must_use]
pub fn generate_dev_replay_events(
    session_id: &str,
    workspace_root: &str,
    scenario: DevReplayScenario,
    turn_index: usize,
) -> Vec<SessionUpdateNotification> {
    let script = build_dev_replay_script(session_id, workspace_root, scenario, turn_index);
    script
        .events
        .into_iter()
        .map(|update| SessionUpdateNotification {
            session_id: session_id.to_owned(),
            update,
        })
        .collect()
}

/// Builds a full [`DevReplayTurnScript`] for a given scenario and turn index.
#[must_use]
pub fn build_dev_replay_script(
    session_id: &str,
    workspace_root: &str,
    scenario: DevReplayScenario,
    turn_index: usize,
) -> DevReplayTurnScript {
    let tc_prefix = format!("replay-{}-t{turn_index}", scenario_slug(scenario));
    match scenario {
        DevReplayScenario::Showcase => DevReplayTurnScript {
            scenario,
            label: "Showcase",
            events: showcase_events(session_id, workspace_root, &tc_prefix),
        },
        DevReplayScenario::TodoList => DevReplayTurnScript {
            scenario,
            label: "Todo list",
            events: todo_list_events(session_id, &tc_prefix),
        },
        DevReplayScenario::Approval => DevReplayTurnScript {
            scenario,
            label: "Write approval",
            events: approval_events(workspace_root, &tc_prefix),
        },
        DevReplayScenario::Question => DevReplayTurnScript {
            scenario,
            label: "Clarification question",
            events: chunk_text_updates(
                "Before applying changes in the workspace, confirm whether to update tests too?",
            ),
        },
        DevReplayScenario::MultiTurn => DevReplayTurnScript {
            scenario,
            label: "Multi-turn",
            events: chunk_text_updates(&format!(
                "Deterministic multi-turn reply number {} for session {session_id}.",
                turn_index + 1
            )),
        },
        DevReplayScenario::CommandOutput => DevReplayTurnScript {
            scenario,
            label: "Command output",
            events: command_output_events(workspace_root, &tc_prefix),
        },
    }
}

const fn scenario_slug(scenario: DevReplayScenario) -> &'static str {
    match scenario {
        DevReplayScenario::Showcase => "showcase",
        DevReplayScenario::TodoList => "todo",
        DevReplayScenario::Approval => "approval",
        DevReplayScenario::Question => "question",
        DevReplayScenario::MultiTurn => "multi",
        DevReplayScenario::CommandOutput => "cmd",
    }
}

fn chunk_text_updates(text: &str) -> Vec<SessionUpdateKind> {
    stream_word_chunks(text, 3)
        .into_iter()
        .map(|chunk| SessionUpdateKind::AgentMessageChunk(PromptContentBlock::Text { text: chunk }))
        .collect()
}

fn showcase_events(
    session_id: &str,
    workspace_root: &str,
    tc_prefix: &str,
) -> Vec<SessionUpdateKind> {
    let read_id = format!("{tc_prefix}-read");
    let edit_id = format!("{tc_prefix}-edit");
    let target_file = format!("{}/README.md", workspace_root.trim_end_matches(['/', '\\']));

    let mut events = vec![
        SessionUpdateKind::AgentThoughtChunk(PromptContentBlock::Text {
            text: materialize_template(
                "Inspecting {workspace} in session {sessionId}.",
                workspace_root,
                session_id,
                &read_id,
            ),
        }),
        SessionUpdateKind::ToolCall(ToolCallUpdatePayload {
            tool_call_id: read_id.clone(),
            title: Some(format!("Read {target_file}")),
            kind: Some(ToolCallKind::Read),
            status: Some(ToolCallStatus::InProgress),
            locations: Some(vec![ToolCallLocation {
                path: target_file.clone(),
                line: Some(1),
            }]),
            content: None,
            raw_input: Some(json!({ "path": target_file })),
            raw_output: None,
        }),
        SessionUpdateKind::ToolCallUpdate(ToolCallUpdatePayload {
            tool_call_id: read_id,
            title: None,
            kind: None,
            status: Some(ToolCallStatus::Completed),
            locations: None,
            content: Some(vec![json!({ "text": "# T3 Code Workspace\n" })]),
            raw_input: None,
            raw_output: None,
        }),
        SessionUpdateKind::ToolCall(ToolCallUpdatePayload {
            tool_call_id: edit_id.clone(),
            title: Some("Edit src/lib.rs".to_owned()),
            kind: Some(ToolCallKind::Edit),
            status: Some(ToolCallStatus::Completed),
            locations: Some(vec![ToolCallLocation {
                path: "src/lib.rs".to_owned(),
                line: Some(12),
            }]),
            content: Some(vec![json!({ "text": "Updated greeting header cleanly." })]),
            raw_input: None,
            raw_output: None,
        }),
    ];
    events.extend(chunk_text_updates(
        "Inspection complete and README summary verified for showcase turn.",
    ));
    events
}

fn todo_list_events(session_id: &str, tc_prefix: &str) -> Vec<SessionUpdateKind> {
    let mut events = vec![
        SessionUpdateKind::Plan {
            entries: vec![
                json!({ "id": format!("{tc_prefix}-1"), "content": "Inspect workspace", "status": "completed" }),
                json!({ "id": format!("{tc_prefix}-2"), "content": "Run focused checks", "status": "in_progress" }),
            ],
        },
        SessionUpdateKind::SessionInfoUpdate {
            title: Some(format!("Replay {session_id}")),
            updated_at: None,
        },
    ];
    events.extend(chunk_text_updates(
        "Checklist updated and all verification items tracked.",
    ));
    events
}

fn approval_events(workspace_root: &str, tc_prefix: &str) -> Vec<SessionUpdateKind> {
    let edit_id = format!("{tc_prefix}-write");
    let path = format!("{}/notes.txt", workspace_root.trim_end_matches(['/', '\\']));
    let mut events = vec![
        SessionUpdateKind::ToolCall(ToolCallUpdatePayload {
            tool_call_id: edit_id.clone(),
            title: Some(format!("Write {path}")),
            kind: Some(ToolCallKind::Edit),
            status: Some(ToolCallStatus::Pending),
            locations: Some(vec![ToolCallLocation {
                path,
                line: Some(1),
            }]),
            content: Some(vec![
                json!({ "text": "Pending user approval before writing." }),
            ]),
            raw_input: None,
            raw_output: None,
        }),
        SessionUpdateKind::ToolCallUpdate(ToolCallUpdatePayload {
            tool_call_id: edit_id,
            title: None,
            kind: None,
            status: Some(ToolCallStatus::Completed),
            locations: None,
            content: Some(vec![json!({ "text": "Write approved and applied." })]),
            raw_input: None,
            raw_output: None,
        }),
    ];
    events.extend(chunk_text_updates("Approved file update applied cleanly."));
    events
}

fn command_output_events(workspace_root: &str, tc_prefix: &str) -> Vec<SessionUpdateKind> {
    let cmd_id = format!("{tc_prefix}-exec");
    let mut events = vec![
        SessionUpdateKind::ToolCall(ToolCallUpdatePayload {
            tool_call_id: cmd_id.clone(),
            title: Some("cargo test -p t3-providers".to_owned()),
            kind: Some(ToolCallKind::Execute),
            status: Some(ToolCallStatus::InProgress),
            locations: Some(vec![ToolCallLocation {
                path: workspace_root.to_owned(),
                line: None,
            }]),
            content: Some(vec![json!({ "text": "running tests...\n" })]),
            raw_input: Some(json!({ "command": "cargo test -p t3-providers" })),
            raw_output: None,
        }),
        SessionUpdateKind::ToolCallUpdate(ToolCallUpdatePayload {
            tool_call_id: cmd_id,
            title: None,
            kind: None,
            status: Some(ToolCallStatus::Completed),
            locations: None,
            content: Some(vec![
                json!({ "text": "running tests...\ntest result: ok." }),
            ]),
            raw_input: None,
            raw_output: Some(json!({ "exitCode": 0 })),
        }),
    ];
    events.extend(chunk_text_updates(
        "Command exited cleanly with status zero.",
    ));
    events
}
