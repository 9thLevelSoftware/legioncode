//! Turn-item conversion and Live Activity projection helpers (`BUG-24`).

use legion_contracts::orchestration::{TurnItem, TurnItemKind, TurnItemStatus};
use legion_surfaces::mobile_i18n::{AgentActivityPhase, AgentActivityRowInput};

use crate::state::WebAppState;
use crate::state_types::TimelineMessage;

pub(crate) fn turn_item_to_timeline(item: TurnItem, sequence: u64) -> TimelineMessage {
    let id = item.base.id.as_str().to_owned();
    let status = Some(turn_status_label(item.base.status).to_owned());
    let title = item.base.title;
    let running = matches!(item.base.status, TurnItemStatus::Running);
    match item.kind {
        TurnItemKind::UserMessage { text, .. } => msg_entry(
            id,
            "user",
            "user_message",
            title,
            text,
            (false, sequence, status),
        ),
        TurnItemKind::AssistantMessage {
            text, streaming, ..
        } => msg_entry(
            id,
            "assistant",
            "assistant_message",
            title,
            text,
            (streaming, sequence, status),
        ),
        TurnItemKind::Reasoning { text, streaming } => msg_entry(
            id,
            "assistant",
            "reasoning",
            title.or_else(|| Some("Reasoning".to_owned())),
            text,
            (streaming, sequence, status),
        ),
        TurnItemKind::ProposedPlan {
            markdown,
            streaming,
            ..
        } => msg_entry(
            id,
            "assistant",
            "proposed_plan",
            title.or_else(|| Some("Proposed Plan".to_owned())),
            markdown,
            (streaming, sequence, status),
        ),
        TurnItemKind::SystemNotice { message, .. }
        | TurnItemKind::RunInterruptRequest { message, .. }
        | TurnItemKind::RunInterruptResult { message } => msg_entry(
            id,
            "system",
            "system_notice",
            title,
            message,
            (false, sequence, status),
        ),
        TurnItemKind::Error { failure, .. } => msg_entry(
            id,
            "system",
            "error",
            title.or_else(|| Some("Error".to_owned())),
            failure.message,
            (false, sequence, status),
        ),
        other => tool_item_to_timeline(id, title, other, (running, sequence, status)),
    }
}

fn tool_item_to_timeline(
    id: String,
    title: Option<String>,
    kind: TurnItemKind,
    meta: (bool, u64, Option<String>),
) -> TimelineMessage {
    let (running, sequence, status) = meta;
    match kind {
        TurnItemKind::CommandExecution {
            input,
            output,
            exit_code,
            ..
        } => {
            let suffix = exit_code.map_or(String::new(), |c| format!(" [exit {c}]"));
            msg_entry(
                id,
                "tool",
                "command_execution",
                Some(format!("{input}{suffix}")),
                output.unwrap_or_default(),
                (running, sequence, status),
            )
        }
        TurnItemKind::FileChange {
            file_name,
            additions,
            deletions,
            diff_str,
            ..
        } => msg_entry(
            id,
            "tool",
            "file_change",
            Some(format!(
                "{file_name} (+{} -{})",
                additions.unwrap_or(0),
                deletions.unwrap_or(0)
            )),
            diff_str.unwrap_or_default(),
            (false, sequence, status),
        ),
        other => msg_entry(
            id,
            "tool",
            "turn_item",
            title.or_else(|| Some(format!("{other:?}").chars().take(48).collect())),
            String::new(),
            (running, sequence, status),
        ),
    }
}

fn msg_entry(
    id: String,
    role: &str,
    kind: &str,
    title: Option<String>,
    body: String,
    meta: (bool, u64, Option<String>),
) -> TimelineMessage {
    let (streaming, sequence, status) = meta;
    TimelineMessage {
        id,
        role: role.to_owned(),
        kind: kind.to_owned(),
        title,
        body,
        streaming,
        sequence,
        status,
    }
}

const fn turn_status_label(status: TurnItemStatus) -> &'static str {
    match status {
        TurnItemStatus::Idle => "idle",
        TurnItemStatus::Pending => "pending",
        TurnItemStatus::Running => "running",
        TurnItemStatus::Waiting => "waiting",
        TurnItemStatus::Completed => "completed",
        TurnItemStatus::Failed => "failed",
        TurnItemStatus::Cancelled => "cancelled",
        TurnItemStatus::Interrupted => "interrupted",
    }
}

pub(crate) fn upsert_timeline_message(list: &mut Vec<TimelineMessage>, next: TimelineMessage) {
    if let Some(existing) = list.iter_mut().find(|m| m.id == next.id) {
        *existing = next;
    } else {
        list.push(next);
    }
}

pub(crate) fn refresh_activity_summary(state: &mut WebAppState) {
    let mut rows = Vec::new();
    for thread in state.threads.values().take(5) {
        let project = state
            .projects
            .iter()
            .find(|p| p.id == thread.project_id)
            .map_or_else(|| thread.project_id.clone(), |p| p.title.clone());
        let phase = state
            .timelines
            .get(&thread.id)
            .and_then(|tl| tl.active_run_status.as_deref())
            .and_then(AgentActivityPhase::parse)
            .unwrap_or(if thread.synchronized {
                AgentActivityPhase::Completed
            } else {
                AgentActivityPhase::Starting
            });
        rows.push(AgentActivityRowInput {
            phase,
            title: thread.title.clone(),
            project,
        });
    }
    state.activity_input.active = !rows.is_empty();
    state.activity_input.phase = rows.first().map(|r| r.phase);
    state.activity_input.rows = rows;
}
