//! Translation from ACP [`SessionUpdateKind`] stream events into orchestration domain events.

use serde_json::{Value, json};
use t3_contracts::ids::{PlanId, TurnItemId};
use t3_contracts::orchestration::{
    DomainEvent, DomainEventPayload, PlanArtifact, TurnItemKind, TurnItemStatus,
};
use t3_providers::acp::presentation::{ToolCallKind, ToolCallStatus, ToolCallUpdatePayload};
use t3_providers::acp::protocol::PromptContentBlock;
use t3_providers::acp::updates::SessionUpdateKind;

use crate::error::ServerRuntimeError;
use crate::turn_events::TurnEmitContext;
use crate::turn_events_builders::TurnItemMeta;

impl TurnEmitContext {
    /// Translate one ACP [`SessionUpdateKind`] into domain events.
    pub fn translate_update(
        &mut self,
        update: &SessionUpdateKind,
        now_iso: &str,
    ) -> Result<Vec<DomainEvent>, ServerRuntimeError> {
        let mut out = Vec::new();
        match update {
            SessionUpdateKind::AgentMessageChunk(block) => {
                if let Some(delta) = prompt_block_text(block) {
                    self.assistant_text.push_str(&delta);
                    out.extend(self.build_assistant_message_events(true, now_iso)?);
                }
            }
            SessionUpdateKind::AgentThoughtChunk(block) => {
                if let Some(delta) = prompt_block_text(block) {
                    self.reasoning_text.push_str(&delta);
                    out.push(self.build_reasoning_event(true, now_iso)?);
                }
            }
            SessionUpdateKind::ToolCall(tc) | SessionUpdateKind::ToolCallUpdate(tc) => {
                out.push(self.build_tool_item_event(tc, now_iso)?);
            }
            SessionUpdateKind::Plan { entries } => {
                self.plan_counter = self.plan_counter.saturating_add(1);
                let plan_id = PlanId::new(format!(
                    "plan-{}-{}",
                    self.run_id.as_str(),
                    self.plan_counter
                ))
                .map_err(|e| ServerRuntimeError::Persistence(e.to_string()))?;
                let plan = PlanArtifact {
                    id: plan_id,
                    thread_id: self.thread_id.clone(),
                    run_id: Some(self.run_id.clone()),
                    node_id: self.root_node_id.clone(),
                    status: "active".to_owned(),
                    detail_in_turn_item: Some(false),
                    kind: "todo_list".to_owned(),
                    markdown: None,
                    steps: Some(entries.clone()),
                    explanation: None,
                };
                out.push(self.wrap_event(now_iso, DomainEventPayload::PlanUpdated(plan))?);
            }
            SessionUpdateKind::UserMessageChunk(_)
            | SessionUpdateKind::AvailableCommandsUpdate { .. }
            | SessionUpdateKind::CurrentModeUpdate { .. }
            | SessionUpdateKind::ConfigOptionUpdate { .. }
            | SessionUpdateKind::SessionInfoUpdate { .. }
            | SessionUpdateKind::UsageUpdate { .. }
            | SessionUpdateKind::Unknown { .. } => {}
        }
        Ok(out)
    }

    pub(crate) fn build_reasoning_event(
        &mut self,
        streaming: bool,
        now_iso: &str,
    ) -> Result<DomainEvent, ServerRuntimeError> {
        let item_id = if let Some(existing) = self.reasoning_item_id.clone() {
            existing
        } else {
            let id = TurnItemId::new(format!("item-reasoning-{}", self.run_id.as_str()))
                .map_err(|e| ServerRuntimeError::Persistence(e.to_string()))?;
            self.reasoning_item_id = Some(id.clone());
            id
        };
        let ord = self.alloc_ordinal();
        let item = self.make_turn_item(
            TurnItemMeta {
                id: item_id,
                ordinal: ord,
                status: if streaming {
                    TurnItemStatus::Running
                } else {
                    TurnItemStatus::Completed
                },
                completed_at: (!streaming).then_some(now_iso),
                title: None,
                now_iso,
            },
            TurnItemKind::Reasoning {
                text: self.reasoning_text.clone(),
                streaming,
            },
        );
        self.wrap_event(now_iso, DomainEventPayload::TurnItemUpdated(Box::new(item)))
    }

    pub(crate) fn build_tool_item_event(
        &mut self,
        tc: &ToolCallUpdatePayload,
        now_iso: &str,
    ) -> Result<DomainEvent, ServerRuntimeError> {
        let (item_id, title, ord) = if let Some(entry) = self.tool_items.get(&tc.tool_call_id) {
            entry.clone()
        } else {
            let ord = self.alloc_ordinal();
            let id = TurnItemId::new(format!("item-tool-{}-{}", self.run_id.as_str(), ord))
                .map_err(|e| ServerRuntimeError::Persistence(e.to_string()))?;
            let title = tc.title.clone().unwrap_or_else(|| tc.tool_call_id.clone());
            self.tool_items
                .insert(tc.tool_call_id.clone(), (id.clone(), title.clone(), ord));
            (id, title, ord)
        };
        let status = match tc.status {
            Some(ToolCallStatus::Pending) => TurnItemStatus::Pending,
            Some(ToolCallStatus::InProgress | ToolCallStatus::Unknown(_)) | None => {
                TurnItemStatus::Running
            }
            Some(ToolCallStatus::Completed) => TurnItemStatus::Completed,
            Some(ToolCallStatus::Failed) => TurnItemStatus::Failed,
        };
        let kind = match tc.kind {
            Some(ToolCallKind::Execute) => TurnItemKind::CommandExecution {
                input: tc
                    .raw_input
                    .as_ref()
                    .map_or_else(|| title.clone(), Value::to_string),
                output: tc.raw_output.as_ref().map(Value::to_string),
                output_omitted: None,
                output_indicates_failure: Some(status == TurnItemStatus::Failed),
                exit_code: None,
                monitor: None,
                completion_wake: None,
            },
            _ => TurnItemKind::DynamicTool {
                tool_name: Some(title.clone()),
                viewed_image_path: None,
                input: tc.raw_input.clone().unwrap_or_else(|| json!({})),
                output: tc.raw_output.clone(),
                output_omitted: None,
            },
        };
        let completed_at =
            matches!(status, TurnItemStatus::Completed | TurnItemStatus::Failed).then_some(now_iso);
        let item = self.make_turn_item(
            TurnItemMeta {
                id: item_id,
                ordinal: ord,
                status,
                completed_at,
                title: Some(&title),
                now_iso,
            },
            kind,
        );
        self.wrap_event(now_iso, DomainEventPayload::TurnItemUpdated(Box::new(item)))
    }
}

fn prompt_block_text(block: &PromptContentBlock) -> Option<String> {
    match block {
        PromptContentBlock::Text { text } => Some(text.clone()),
        _ => None,
    }
}
