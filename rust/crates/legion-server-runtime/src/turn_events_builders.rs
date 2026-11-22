//! Internal domain-model builders for [`crate::turn_events::TurnEmitContext`].

use legion_contracts::ids::TurnItemId;
use legion_contracts::orchestration::{
    ConversationMessage, CreationSource, DomainEvent, DomainEventPayload, ExecutionNode,
    OrchestrationActor, ProviderSession, ProviderThread, ProviderTurn, TurnItem, TurnItemBase,
    TurnItemKind, TurnItemStatus, default_wire_provider_capabilities,
};

use crate::error::ServerRuntimeError;
use crate::turn_events::TurnEmitContext;

pub(crate) struct TurnItemMeta<'a> {
    pub id: TurnItemId,
    pub ordinal: u64,
    pub status: TurnItemStatus,
    pub completed_at: Option<&'a str>,
    pub title: Option<&'a str>,
    pub now_iso: &'a str,
}

impl TurnEmitContext {
    pub(crate) fn make_turn_item(&self, meta: TurnItemMeta<'_>, kind: TurnItemKind) -> TurnItem {
        let base = TurnItemBase {
            deferred_work: None,
            tool_non_execution_kind: None,
            tool_surface: None,
            tool_icon: None,
            tool_source: None,
            id: meta.id,
            thread_id: self.thread_id.clone(),
            run_id: Some(self.run_id.clone()),
            node_id: Some(self.root_node_id.clone()),
            provider_thread_id: Some(self.provider_thread_id.clone()),
            provider_turn_id: Some(self.provider_turn_id.clone()),
            native_item_ref: None,
            parent_item_id: None,
            ordinal: meta.ordinal,
            status: meta.status,
            title: meta.title.map(ToOwned::to_owned),
            started_at: Some(self.started_at.clone()),
            completed_at: meta.completed_at.map(ToOwned::to_owned),
            updated_at: meta.now_iso.to_owned(),
        };
        TurnItem { base, kind }
    }

    pub(crate) fn build_assistant_message_events(
        &mut self,
        streaming: bool,
        now_iso: &str,
    ) -> Result<Vec<DomainEvent>, ServerRuntimeError> {
        let ord = self.alloc_ordinal();
        let msg = ConversationMessage {
            notification: None,
            created_by: OrchestrationActor::Agent,
            creation_source: CreationSource::Server,
            scheduled_task_id: None,
            follows_thread_selection: None,
            sender_thread_id: None,
            linked_sender: None,
            idle_compaction: None,
            workflow_script: None,
            id: self.assistant_message_id.clone(),
            thread_id: self.thread_id.clone(),
            run_id: Some(self.run_id.clone()),
            node_id: Some(self.root_node_id.clone()),
            role: "assistant".to_owned(),
            text: self.assistant_text.clone(),
            context: None,
            attachments: Vec::new(),
            streaming,
            created_at: self.started_at.clone(),
            updated_at: now_iso.to_owned(),
            delegated_completion: None,
        };
        let status = if streaming {
            TurnItemStatus::Running
        } else {
            TurnItemStatus::Completed
        };
        let item = self.make_turn_item(
            TurnItemMeta {
                id: self.assistant_item_id.clone(),
                ordinal: ord,
                status,
                completed_at: (!streaming).then_some(now_iso),
                title: None,
                now_iso,
            },
            TurnItemKind::AssistantMessage {
                phase: None,
                message_id: self.assistant_message_id.clone(),
                text: self.assistant_text.clone(),
                text_omitted: None,
                attachments: None,
                streaming,
            },
        );
        Ok(vec![
            self.wrap_event(now_iso, DomainEventPayload::MessageUpdated(Box::new(msg)))?,
            self.wrap_event(now_iso, DomainEventPayload::TurnItemUpdated(Box::new(item)))?,
        ])
    }

    pub(crate) fn build_provider_session(
        &self,
        status: &str,
        model: &str,
        created_at: &str,
        updated_at: &str,
    ) -> ProviderSession {
        ProviderSession {
            id: self.provider_session_id.clone(),
            driver: self.driver.clone(),
            provider_instance_id: self.provider_instance_id.clone(),
            status: status.to_owned(),
            cwd: ".".to_owned(),
            model: Some(model.to_owned()),
            capabilities: default_wire_provider_capabilities(),
            extension_statuses: Vec::new(),
            created_at: created_at.to_owned(),
            updated_at: updated_at.to_owned(),
            last_error: None,
        }
    }

    pub(crate) fn build_provider_thread(
        &self,
        status: &str,
        ordinal: u64,
        now_iso: &str,
    ) -> ProviderThread {
        ProviderThread {
            id: self.provider_thread_id.clone(),
            driver: self.driver.clone(),
            provider_instance_id: self.provider_instance_id.clone(),
            provider_session_id: Some(self.provider_session_id.clone()),
            app_thread_id: Some(self.thread_id.clone()),
            owner_node_id: Some(self.root_node_id.clone()),
            native_thread_ref: None,
            native_conversation_head_ref: None,
            status: status.to_owned(),
            first_run_ordinal: Some(ordinal),
            last_run_ordinal: Some(ordinal),
            handoff_ids: Vec::new(),
            forked_from: None,
            pending_background_tasks: Vec::new(),
            context_usage: None,
            native_metadata: None,
            goal: None,
            created_at: self.started_at.clone(),
            updated_at: now_iso.to_owned(),
        }
    }

    pub(crate) fn build_provider_turn(
        &self,
        status: &str,
        ordinal: u64,
        started_at: Option<&str>,
        completed_at: Option<&str>,
    ) -> ProviderTurn {
        ProviderTurn {
            id: self.provider_turn_id.clone(),
            provider_thread_id: self.provider_thread_id.clone(),
            node_id: self.root_node_id.clone(),
            run_attempt_id: self.attempt_id.clone(),
            native_turn_ref: None,
            native_thread_ref: None,
            ordinal,
            status: status.to_owned(),
            started_at: started_at.map(ToOwned::to_owned),
            completed_at: completed_at.map(ToOwned::to_owned),
            token_usage: None,
            turn_token_usage: None,
            model: None,
        }
    }

    pub(crate) fn build_execution_node(
        &self,
        status: &str,
        started_at: Option<&str>,
        completed_at: Option<&str>,
    ) -> ExecutionNode {
        ExecutionNode {
            id: self.root_node_id.clone(),
            thread_id: self.thread_id.clone(),
            run_id: Some(self.run_id.clone()),
            parent_node_id: None,
            root_node_id: self.root_node_id.clone(),
            kind: "root_turn".to_owned(),
            status: status.to_owned(),
            counts_for_run: true,
            provider_thread_id: Some(self.provider_thread_id.clone()),
            provider_turn_id: Some(self.provider_turn_id.clone()),
            native_item_ref: None,
            runtime_request_id: None,
            checkpoint_scope_id: None,
            started_at: started_at.map(ToOwned::to_owned),
            completed_at: completed_at.map(ToOwned::to_owned),
        }
    }
}
