//! `Replay (dev)` provider turn driver translating deterministic replay events into orchestration commits.

use t3_contracts::ids::{
    CheckpointScopeId, CommandId, ProviderDriverKind, ProviderSessionId, ProviderThreadId,
};
use t3_contracts::orchestration::{Run, RunAttempt, RunStatus};
use t3_orchestrator::outbox::{OutboxEffectRequest, PendingOutboxEffect};
use t3_orchestrator::sink::EventSink;
use t3_providers::drivers::dev_replay::{DevReplayScenario, generate_dev_replay_events};
use uuid::Uuid;

use crate::error::ServerRuntimeError;
use crate::turn_events::{TurnEmitContext, commit_runtime_events};

/// Inputs needed to run a deterministic `Replay (dev)` turn.
#[derive(Debug, Clone)]
pub struct ReplayTurnInput {
    /// Active run record.
    pub run: Run,
    /// Active attempt record if present.
    pub attempt: Option<RunAttempt>,
    /// Workspace root path passed to the dev-replay generator.
    pub workspace_root: String,
    /// User prompt text used to select the scenario.
    pub prompt: String,
    /// Optional provider thread ID to attach.
    pub provider_thread_id: Option<ProviderThreadId>,
    /// Replay scenario override; derived from `prompt` if `None`.
    pub scenario: Option<DevReplayScenario>,
    /// ISO-8601 timestamp base.
    pub now_iso: String,
    /// Checkpoint scope identifier queued for capture at turn completion.
    pub scope_id: CheckpointScopeId,
    /// Starting ordinal for emitted turn items.
    pub next_item_ordinal: u64,
    /// Whether to queue `ThreadTitleGenerate` at turn completion.
    pub queue_title_generate: bool,
}

/// Choose a [`DevReplayScenario`] cleanly from prompt text.
pub fn select_dev_replay_scenario(prompt: &str) -> DevReplayScenario {
    let lower = prompt.to_ascii_lowercase();
    if lower.contains("todo") {
        DevReplayScenario::TodoList
    } else if lower.contains("command") || lower.contains("tool") {
        DevReplayScenario::CommandOutput
    } else if lower.contains("approval") {
        DevReplayScenario::Approval
    } else if lower.contains("question") {
        DevReplayScenario::Question
    } else if lower.contains("multi") {
        DevReplayScenario::MultiTurn
    } else {
        DevReplayScenario::Showcase
    }
}

/// Execute a `Replay (dev)` turn via [`generate_dev_replay_events`] and commit its events + follow-up outbox effects.
pub fn execute_dev_replay_turn(
    sink: &EventSink,
    input: &ReplayTurnInput,
) -> Result<(), ServerRuntimeError> {
    let driver = ProviderDriverKind::new("acp")
        .map_err(|e| ServerRuntimeError::Persistence(e.to_string()))?;
    let session_id = ProviderSessionId::new(format!("psess-{}", input.run.id.as_str()))
        .map_err(|e| ServerRuntimeError::Persistence(e.to_string()))?;
    let p_thread_id = match input.provider_thread_id.clone() {
        Some(existing) => existing,
        None => ProviderThreadId::new(format!("pthread-{}", input.run.thread_id.as_str()))
            .map_err(|e| ServerRuntimeError::Persistence(e.to_string()))?,
    };

    let scenario = input
        .scenario
        .unwrap_or_else(|| select_dev_replay_scenario(&input.prompt));
    let turn_index = usize::try_from(input.run.ordinal.saturating_sub(1)).unwrap_or(0);
    let updates = generate_dev_replay_events(
        session_id.as_str(),
        &input.workspace_root,
        scenario,
        turn_index,
    );

    let mut ctx = TurnEmitContext::new(
        &input.run,
        driver,
        session_id,
        p_thread_id,
        input.now_iso.clone(),
        input.next_item_ordinal,
    )?;

    let start_events =
        ctx.build_start_events(&input.run, input.attempt.as_ref(), &input.now_iso)?;
    commit_runtime_events(
        sink,
        &input.run.thread_id,
        "runtime.replay.turn-started",
        &input.now_iso,
        start_events,
        Vec::new(),
    )?;

    let mut stream_events = Vec::new();
    for notif in &updates {
        stream_events.extend(ctx.translate_update(&notif.update, &input.now_iso)?);
    }
    if !stream_events.is_empty() {
        commit_runtime_events(
            sink,
            &input.run.thread_id,
            "runtime.replay.stream",
            &input.now_iso,
            stream_events,
            Vec::new(),
        )?;
    }

    let finish_events = ctx.build_finish_events(
        &input.run,
        input.attempt.as_ref(),
        RunStatus::Completed,
        &input.now_iso,
    )?;
    let cmd_id = CommandId::new(format!("cmd-followup-{}", Uuid::new_v4()))
        .map_err(|e| ServerRuntimeError::Persistence(e.to_string()))?;
    let mut followups = vec![PendingOutboxEffect {
        id: format!("eff-cp-{}", Uuid::new_v4()),
        command_id: cmd_id.clone(),
        thread_id: input.run.thread_id.clone(),
        request: OutboxEffectRequest::CheckpointCapture {
            run_id: input.run.id.clone(),
            scope_id: input.scope_id.clone(),
        },
        available_at: None,
    }];
    if input.queue_title_generate {
        followups.push(PendingOutboxEffect {
            id: format!("eff-title-{}", Uuid::new_v4()),
            command_id: cmd_id,
            thread_id: input.run.thread_id.clone(),
            request: OutboxEffectRequest::ThreadTitleGenerate {
                kind: "initial".to_owned(),
                message_id: Some(input.run.user_message_id.clone()),
            },
            available_at: None,
        });
    }

    commit_runtime_events(
        sink,
        &input.run.thread_id,
        "runtime.replay.turn-completed",
        &input.now_iso,
        finish_events,
        followups,
    )
}
