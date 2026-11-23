//! Runtime `WorkflowMilestoneSink`, `WorkflowAgentDriver`, and `OrchestrationMilestones` pump
//! bridging `legion-workflows` with `Orchestrator` and `MilestoneHub` (`#38882`, `#40774`).

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use legion_contracts::ids::{CommandId, MessageId, ProjectId, ProviderInstanceId, ThreadId};
use legion_contracts::orchestration::{
    CreationSource, DispatchMode, InitialThreadMessage, MessageDispatchCommand, ModelSelection,
    OrchestrationActor, OrchestrationCommand, ProviderInteractionMode, ThreadCreateCommand,
    ThreadStopCommand,
};
use legion_contracts::policy::RuntimeMode;
use legion_orchestrator::Orchestrator;
use legion_orchestrator::decider::title_ops::decide_thread_create_with_title_renamed;
use legion_orchestrator::locks::OrchestratorLocks;
use legion_orchestrator::sink::{CommitCommandInput, projection_apply::load_decider_state};
use legion_persistence::project_store::{find_project_by_id_or_workspace, list_projects};
use legion_workflows::traits::{
    AgentInterruptOutcome, AgentInterruptTarget, AgentLaunch, AgentModel, AgentResult,
    AgentStartInput, AgentTurn, BoxFuture, WorkflowAgentDriver, WorkflowAgentError,
};
use serde_json::json;
use uuid::Uuid;

use crate::outbox_state::utc_now_iso;
use crate::workflow_driver_pump::{latest_run_id, read_turn_settlement};

/// Dispatches workflow `agent` steps against the local [`Orchestrator`].
#[derive(Clone)]
pub struct OrchestratorWorkflowAgentDriver {
    orchestrator: Arc<Orchestrator>,
}

impl OrchestratorWorkflowAgentDriver {
    /// Creates an agent driver backed by `orchestrator`.
    pub fn new(orchestrator: Arc<Orchestrator>) -> Self {
        Self { orchestrator }
    }

    fn resolve_project_id(&self, hint: Option<&str>) -> Result<ProjectId, WorkflowAgentError> {
        let conn = self
            .orchestrator
            .sink()
            .db()
            .lock()
            .map_err(|_| WorkflowAgentError::new("database lock poisoned"))?;
        if let Some(raw) = hint.map(str::trim).filter(|s| !s.is_empty())
            && let Some(found) = find_project_by_id_or_workspace(&*conn, raw)
                .map_err(|e| WorkflowAgentError::new(e.to_string()))?
        {
            return ProjectId::new(found.id).map_err(|e| WorkflowAgentError::new(e.to_string()));
        }
        let projects = list_projects(&*conn).map_err(|e| WorkflowAgentError::new(e.to_string()))?;
        let first = projects
            .into_iter()
            .find(|p| p.deleted_at.is_none())
            .ok_or_else(|| {
                WorkflowAgentError::new("No active project available for workflow agent step.")
            })?;
        ProjectId::new(first.id).map_err(|e| WorkflowAgentError::new(e.to_string()))
    }

    fn convert_model_selection(m: &AgentModel) -> Result<ModelSelection, WorkflowAgentError> {
        Ok(ModelSelection {
            instance_id: ProviderInstanceId::new(&m.instance_id)
                .map_err(|e| WorkflowAgentError::new(e.to_string()))?,
            model: m.model.clone(),
            options: m
                .options
                .as_ref()
                .and_then(|v| serde_json::from_value(v.clone()).ok()),
        })
    }

    async fn start_on_existing_thread(
        &self,
        input: AgentStartInput,
        existing_tid: &str,
        msg_id: MessageId,
        now_iso: &str,
        started_ms: i64,
    ) -> Result<AgentTurn, WorkflowAgentError> {
        let tid =
            ThreadId::new(existing_tid).map_err(|e| WorkflowAgentError::new(e.to_string()))?;
        let cmd_id = CommandId::new(format!("wf-send-{}", Uuid::new_v4()))
            .map_err(|e| WorkflowAgentError::new(e.to_string()))?;
        let model_selection = input
            .model
            .as_ref()
            .and_then(|m| Self::convert_model_selection(m).ok());
        let cmd = OrchestrationCommand::MessageDispatch(MessageDispatchCommand {
            notification: None,
            guest_message_id: None,
            created_by: OrchestrationActor::System,
            creation_source: CreationSource::Web,
            scheduled_task_id: None,
            sender_thread_id: None,
            linked_sender: None,
            idle_compaction: None,
            fresh_context: None,
            output_schema: input.output_schema,
            command_id: cmd_id,
            thread_id: tid.clone(),
            message_id: msg_id,
            text: input.prompt,
            context: None,
            attachments: Vec::new(),
            title_seed: None,
            model_selection,
            source_plan_ref: None,
            restart_continuation_of_run_id: None,
            usage_limit_continuation_of_run_id: None,
            manual_continuation_of_run_id: None,
            steer_follow_up_of_run_id: None,
            steer_follow_up_after_run_ordinal: None,
            usage_limit_recovery_request_id: None,
            delivery_intent: None,
            delegated_completion: None,
            dispatch_mode: DispatchMode::StartImmediately,
        });
        self.orchestrator
            .dispatch_command(&cmd, now_iso)
            .await
            .map_err(|e| WorkflowAgentError::new(e.to_string()))?;
        let turn_id = latest_run_id(&self.orchestrator, tid.as_str()).unwrap_or_default();
        Ok(AgentTurn {
            thread_id: tid.as_str().to_owned(),
            turn_id,
            environment: input.environment,
            started_at: Some(started_ms),
            failed: None,
        })
    }

    fn parse_launch_modes(launch: Option<&AgentLaunch>) -> (RuntimeMode, ProviderInteractionMode) {
        let runtime_mode = match launch.and_then(|l| l.runtime_mode.as_deref()) {
            Some("approval-required") => RuntimeMode::ApprovalRequired,
            Some("auto-accept-edits") => RuntimeMode::AutoAcceptEdits,
            Some("auto") => RuntimeMode::Auto,
            _ => RuntimeMode::FullAccess,
        };
        let interaction_mode = match launch.and_then(|l| l.interaction_mode.as_deref()) {
            Some("plan") => ProviderInteractionMode::Plan,
            _ => ProviderInteractionMode::Default,
        };
        (runtime_mode, interaction_mode)
    }

    fn start_on_new_thread(
        &self,
        input: AgentStartInput,
        msg_id: MessageId,
        now_iso: &str,
        started_ms: i64,
    ) -> Result<AgentTurn, WorkflowAgentError> {
        let project_id = self.resolve_project_id(input.project.as_deref())?;
        let tid = ThreadId::new(format!("wf-thr-{}", Uuid::new_v4()))
            .map_err(|e| WorkflowAgentError::new(e.to_string()))?;
        let cmd_id = CommandId::new(format!("wf-create-{}", Uuid::new_v4()))
            .map_err(|e| WorkflowAgentError::new(e.to_string()))?;
        let model_sel = match input.model.as_ref() {
            Some(m) => Self::convert_model_selection(m)?,
            None => ModelSelection {
                instance_id: ProviderInstanceId::new("codex")
                    .map_err(|e| WorkflowAgentError::new(e.to_string()))?,
                model: "gpt-5".to_owned(),
                options: None,
            },
        };
        let (runtime_mode, interaction_mode) = Self::parse_launch_modes(input.launch.as_ref());
        let create_cmd = ThreadCreateCommand {
            created_by: OrchestrationActor::System,
            creation_source: CreationSource::Web,
            command_id: cmd_id.clone(),
            thread_id: tid.clone(),
            project_id,
            title: input.title,
            generate_title: Some(false),
            model_selection: model_sel,
            runtime_mode,
            interaction_mode,
            branch: None,
            worktree_path: None,
            link_origin: None,
            delegated_from: None,
            imported_native_thread: None,
            initial_message: Some(InitialThreadMessage {
                message_id: msg_id,
                text: input.prompt,
                attachments: Vec::new(),
                context: None,
                title_seed: None,
                scheduled_task_id: None,
                sender_thread_id: None,
                output_schema: input.output_schema,
                workspace_strategy: None,
            }),
        };
        let state = {
            let conn = self
                .orchestrator
                .sink()
                .db()
                .lock()
                .map_err(|_| WorkflowAgentError::new("database lock poisoned"))?;
            load_decider_state(&conn, &tid).map_err(|e| WorkflowAgentError::new(e.to_string()))?
        };
        let decision = decide_thread_create_with_title_renamed(&state, &create_cmd, true, now_iso)
            .map_err(|e| WorkflowAgentError::new(e.to_string()))?;
        let _session = OrchestratorLocks::new().session();
        let _ = self
            .orchestrator
            .sink()
            .commit_command(&CommitCommandInput {
                command_id: cmd_id,
                command_type: "thread.create".to_owned(),
                thread_id: tid.clone(),
                actor_kind: "system".to_owned(),
                causation_event_id: None,
                correlation_id: None,
                now_iso: now_iso.to_owned(),
                events: decision.events,
                outbox_effects: decision.outbox_effects,
            })
            .map_err(|e| WorkflowAgentError::new(e.to_string()))?;
        if let Ok(conn) = self.orchestrator.sink().db().lock() {
            let origin = json!({
                "environmentId": input.environment.clone().unwrap_or_else(|| "local".to_owned()),
                "runId": input.origin.run_id,
                "workflow": input.origin.workflow,
                "step": input.origin.step,
            });
            let _ = conn.execute(
                "UPDATE orchestration_v2_projection_threads SET payload_json = json_set(payload_json, '$.workflowRun', json(?1)) WHERE thread_id = ?2",
                rusqlite::params![origin.to_string(), tid.as_str()],
            );
        }
        let turn_id = latest_run_id(&self.orchestrator, tid.as_str()).unwrap_or_default();
        Ok(AgentTurn {
            thread_id: tid.as_str().to_owned(),
            turn_id,
            environment: input.environment,
            started_at: Some(started_ms),
            failed: None,
        })
    }
}

impl WorkflowAgentDriver for OrchestratorWorkflowAgentDriver {
    fn start(
        &self,
        input: AgentStartInput,
    ) -> BoxFuture<'_, Result<AgentTurn, WorkflowAgentError>> {
        Box::pin(async move {
            let now_iso = utc_now_iso();
            let started_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
            let msg_id = MessageId::new(format!("wf-msg-{}", Uuid::new_v4()))
                .map_err(|e| WorkflowAgentError::new(e.to_string()))?;
            if let Some(existing_tid) = input.thread.as_deref().map(str::to_owned) {
                self.start_on_existing_thread(input, &existing_tid, msg_id, &now_iso, started_ms)
                    .await
            } else {
                self.start_on_new_thread(input, msg_id, &now_iso, started_ms)
            }
        })
    }

    fn result(
        &self,
        turn: AgentTurn,
        quiet_ms: Option<i64>,
    ) -> BoxFuture<'_, Result<AgentResult, WorkflowAgentError>> {
        Box::pin(async move {
            let mut sub = self.orchestrator.subscribe();
            loop {
                if let Some(res) =
                    read_turn_settlement(&self.orchestrator, &turn.thread_id, &turn.turn_id)?
                {
                    if let Some(ms) = quiet_ms.filter(|&m| m > 0) {
                        tokio::time::sleep(Duration::from_millis(
                            u64::try_from(ms).unwrap_or(1_000).min(5_000),
                        ))
                        .await;
                    }
                    return Ok(res);
                }
                if sub.recv().await.is_none() {
                    return Err(WorkflowAgentError::new(
                        "Orchestrator event stream closed before turn settled.",
                    ));
                }
            }
        })
    }

    fn interrupt(
        &self,
        target: AgentInterruptTarget,
    ) -> BoxFuture<'_, Result<AgentInterruptOutcome, WorkflowAgentError>> {
        Box::pin(async move {
            let Ok(tid) = ThreadId::new(&target.thread_id) else {
                return Ok(AgentInterruptOutcome::Ended);
            };
            let cmd_id = CommandId::new(format!("wf-stop-{}", Uuid::new_v4()))
                .map_err(|e| WorkflowAgentError::new(e.to_string()))?;
            let cmd = OrchestrationCommand::ThreadStop(ThreadStopCommand {
                command_id: cmd_id,
                thread_id: tid,
                reason: None,
                stop_delegated_tasks: Some(true),
            });
            match self.orchestrator.dispatch_now(&cmd).await {
                Ok(_) => Ok(AgentInterruptOutcome::Stopped),
                Err(_) => Ok(AgentInterruptOutcome::Ended),
            }
        })
    }
}
