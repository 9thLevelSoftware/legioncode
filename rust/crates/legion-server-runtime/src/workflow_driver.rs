//! Runtime `WorkflowMilestoneSink`, `WorkflowAgentDriver`, and `OrchestrationMilestones` pump
//! bridging `legion-workflows` with `Orchestrator` and `MilestoneHub` (`#38882`, `#40774`).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use legion_contracts::ids::{
    CommandId, EnvironmentId, MessageId, ProviderInstanceId, RunId, ThreadId,
};
use legion_contracts::orchestration::{
    CreationSource, DispatchMode, InitialThreadMessage, MessageDispatchCommand, ModelSelection,
    OrchestrationActor, OrchestrationCommand, ProviderInteractionMode, ProviderOptionSelection,
    RunInterruptCommand, ThreadStopCommand, WorkflowOrigin, WorkspaceStrategy,
};
use legion_contracts::policy::RuntimeMode;
use legion_orchestrator::decider::DeciderError;
use legion_orchestrator::{Orchestrator, OrchestratorError};
use legion_workflows::traits::{
    AgentInterruptOutcome, AgentInterruptTarget, AgentLaunch, AgentModel, AgentResult,
    AgentStartInput, AgentTurn, BoxFuture, WorkflowAgentDriver, WorkflowAgentError,
};
use serde_json::Value;
use uuid::Uuid;

use crate::outbox_state::utc_now_iso;
use crate::thread_launcher::{
    LaunchThreadRequest, ThreadLauncher, parse_interaction_mode_strict, parse_runtime_mode_strict,
};
use crate::workflow_driver_pump::{latest_run_id, read_turn_settlement};

/// Dispatches workflow `agent` steps against the local [`Orchestrator`] via [`ThreadLauncher`].
#[derive(Clone)]
pub struct OrchestratorWorkflowAgentDriver {
    launcher: ThreadLauncher,
    orchestrator: Arc<Orchestrator>,
    default_environment_id: EnvironmentId,
}

impl OrchestratorWorkflowAgentDriver {
    /// Creates an agent driver backed by `orchestrator` and `worktrees_dir`.
    pub fn new(orchestrator: Arc<Orchestrator>, worktrees_dir: PathBuf) -> Self {
        let launcher = ThreadLauncher::new(Arc::clone(&orchestrator), worktrees_dir);
        let default_environment_id =
            EnvironmentId::new("local").expect("valid constant environment id");
        Self {
            launcher,
            orchestrator,
            default_environment_id,
        }
    }

    /// Attaches an optional `settings.json` path to the underlying [`ThreadLauncher`].
    #[must_use]
    pub fn with_settings_path(mut self, settings_path: Option<PathBuf>) -> Self {
        self.launcher = self.launcher.with_settings_path(settings_path);
        self
    }

    /// Sets the host environment ID recorded in `WorkflowOrigin` when `input.environment` is omitted.
    #[must_use]
    pub fn with_environment_id(mut self, environment_id: EnvironmentId) -> Self {
        self.default_environment_id = environment_id;
        self
    }

    fn convert_model_selection(m: &AgentModel) -> Result<ModelSelection, WorkflowAgentError> {
        let instance_id = ProviderInstanceId::new(&m.instance_id)
            .map_err(|e| WorkflowAgentError::new(e.to_string()))?;
        let model = m.model.trim().to_owned();
        if model.is_empty() {
            return Err(WorkflowAgentError::new("Model identifier cannot be empty."));
        }
        let options = match &m.options {
            None | Some(Value::Null) => None,
            Some(raw) => {
                let parsed = serde_json::from_value::<Vec<ProviderOptionSelection>>(raw.clone())
                    .map_err(|e| WorkflowAgentError::new(format!("Invalid model options: {e}")))?;
                Some(parsed)
            }
        };
        Ok(ModelSelection {
            instance_id,
            model,
            options,
        })
    }

    fn parse_launch_modes(
        launch: Option<&AgentLaunch>,
    ) -> Result<(RuntimeMode, ProviderInteractionMode), WorkflowAgentError> {
        let raw_runtime = launch
            .and_then(|l| l.runtime_mode.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                WorkflowAgentError::new("Missing required runtimeMode on workflow agent launch.")
            })?;
        let runtime_mode = parse_runtime_mode_strict(raw_runtime)
            .map_err(|e| WorkflowAgentError::new(e.to_string()))?;
        let interaction_mode = match launch.and_then(|l| l.interaction_mode.as_deref()) {
            Some(raw) => parse_interaction_mode_strict(raw)
                .map_err(|e| WorkflowAgentError::new(e.to_string()))?,
            None => ProviderInteractionMode::Default,
        };
        Ok((runtime_mode, interaction_mode))
    }

    fn parse_workspace_strategy(
        launch: Option<&AgentLaunch>,
    ) -> Result<WorkspaceStrategy, WorkflowAgentError> {
        let Some(raw_val) = launch.and_then(|l| l.workspace.as_ref()) else {
            return Ok(WorkspaceStrategy::Root { branch: None });
        };
        if raw_val.is_null() {
            return Ok(WorkspaceStrategy::Root { branch: None });
        }
        let mut normalized = raw_val.clone();
        if let Some(obj) = normalized.as_object_mut()
            && obj.get("type").and_then(Value::as_str) == Some("worktree")
            && !obj.contains_key("baseRef")
        {
            obj.insert("baseRef".to_owned(), Value::String("HEAD".to_owned()));
        }
        serde_json::from_value::<WorkspaceStrategy>(normalized).map_err(|e| {
            WorkflowAgentError::new(format!("Invalid workflow launch workspace strategy: {e}"))
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
            .map(Self::convert_model_selection)
            .transpose()?;
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

    async fn start_on_new_thread(
        &self,
        input: AgentStartInput,
        msg_id: MessageId,
        started_ms: i64,
    ) -> Result<AgentTurn, WorkflowAgentError> {
        let project_hint = input
            .project
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                WorkflowAgentError::new("Missing required project for workflow agent step.")
            })?
            .to_owned();
        let model_agent = input.model.as_ref().ok_or_else(|| {
            WorkflowAgentError::new("Missing required model selection for workflow agent step.")
        })?;
        let model_selection = Self::convert_model_selection(model_agent)?;
        let (runtime_mode, interaction_mode) = Self::parse_launch_modes(input.launch.as_ref())?;
        let workspace_strategy = Self::parse_workspace_strategy(input.launch.as_ref())?;

        let stable_launch_key = input
            .stable_launch_key
            .as_deref()
            .or_else(|| {
                input
                    .launch
                    .as_ref()
                    .and_then(|l| l.stable_launch_key.as_deref())
            })
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map_or_else(
                || format!("wf-step:{}:{}", input.origin.run_id, input.origin.step),
                str::to_owned,
            );

        let env_id = match input
            .environment
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(raw) => {
                EnvironmentId::new(raw).map_err(|e| WorkflowAgentError::new(e.to_string()))?
            }
            None => self.default_environment_id.clone(),
        };

        let cmd_id = CommandId::new(format!("wf-create-{}", Uuid::new_v4()))
            .map_err(|e| WorkflowAgentError::new(e.to_string()))?;

        let launched = self
            .launcher
            .launch_thread(LaunchThreadRequest {
                command_id: cmd_id,
                created_by: OrchestrationActor::System,
                creation_source: CreationSource::Server,
                thread_id: None,
                stable_launch_key: Some(stable_launch_key),
                reuse_existing_thread: true,
                project_id: project_hint,
                title: input.title,
                title_renamed: Some(true),
                generate_title: Some(false),
                model_selection,
                runtime_mode,
                interaction_mode,
                workspace_strategy,
                workflow_run: Some(WorkflowOrigin {
                    environment_id: env_id,
                    run_id: input.origin.run_id,
                    workflow: input.origin.workflow,
                    step: input.origin.step,
                }),
                link_origin: None,
                delegated_from: None,
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
            })
            .await
            .map_err(|e| WorkflowAgentError::new(e.to_string()))?;

        Ok(AgentTurn {
            thread_id: launched.thread_id.as_str().to_owned(),
            turn_id: launched.run_id.unwrap_or_default(),
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
                self.start_on_new_thread(input, msg_id, started_ms).await
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

            if let Some(rid_raw) = target
                .turn_id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                let Ok(run_id) = RunId::new(rid_raw) else {
                    return Ok(AgentInterruptOutcome::Ended);
                };
                let cmd = OrchestrationCommand::RunInterrupt(RunInterruptCommand {
                    command_id: cmd_id,
                    thread_id: tid,
                    run_id,
                    reason: Some("Interrupted by workflow engine".to_owned()),
                    hold_queue: None,
                });
                return match self.orchestrator.dispatch_now(&cmd).await {
                    Ok(_) => Ok(AgentInterruptOutcome::Stopped),
                    Err(OrchestratorError::Decider(
                        DeciderError::InvalidRunState(_)
                        | DeciderError::ThreadNotFound(_)
                        | DeciderError::ThreadDeleted(_),
                    )) => Ok(AgentInterruptOutcome::Ended),
                    Err(err) => Err(WorkflowAgentError::new(err.to_string())),
                };
            }

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
