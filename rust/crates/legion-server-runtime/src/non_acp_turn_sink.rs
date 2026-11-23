//! Normalized translation from `legion_providers::non_acp::NonAcpTurnUpdate` into `Orchestrator` commits.

use legion_contracts::ids::TurnItemId;
use legion_contracts::orchestration::{
    DomainEvent, DomainEventPayload, TurnItemKind, TurnItemStatus,
};
use legion_providers::drivers::claude_lifecycle::{
    ClaudeLostBackgroundWorkPlan, ClaudeModelSwitchRestartCheck, ClaudeThinkingTracker,
    ClaudeTrackedBackgroundTask, resolve_claude_lost_background_work,
    should_start_updated_claude_on_model_switch,
};
use legion_providers::drivers::codex_lifecycle::CodexAppServerRetirementState;
use legion_providers::non_acp::NonAcpTurnUpdate;
use serde_json::{Value, json};

use crate::error::ServerRuntimeError;
use crate::turn_events::TurnEmitContext;
use crate::turn_events_builders::TurnItemMeta;

/// Accumulated turn state derived from a stream of [`NonAcpTurnUpdate`] items.
#[derive(Debug, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "Tracks independent per-turn stream suppression and thinking block state flags."
)]
pub struct NonAcpStreamSummary {
    /// Native provider session identifier reported by `SessionStarted`, if any.
    pub native_session_id: Option<String>,
    /// Failure message reported by `TurnFailed`, if any.
    pub failure_message: Option<String>,
    /// Reported token usage `(input, output)` if received.
    pub usage: Option<(u64, u64)>,
    /// Whether `TurnCompleted` or `TurnFailed` was observed.
    pub terminal_seen: bool,
    /// Orchestration run ordinal attributed to generated provider turns.
    pub run_ordinal: u64,
    /// Tracks whether streamed `text_delta` chunks were emitted for the current Claude message block.
    pub saw_stream_text_delta: bool,
    /// Tracks whether streamed `thinking_delta` chunks were emitted for the current Claude message block.
    pub saw_stream_thinking_delta: bool,
    /// Tracks Claude `thinking` blocks per message for pre-tool update surfacing (`#41085`).
    pub thinking_tracker: ClaudeThinkingTracker,
    /// Buffered text of the currently open Claude thinking block.
    pub current_thinking_text: String,
    /// True while inside an active thinking content block.
    pub thinking_block_open: bool,
    /// Monotonic counter identifying completed thinking blocks within the turn.
    pub thinking_seq: u64,
    /// App-server retirement state preventing reuse after terminal failures or deadline errors (`#25123`).
    pub codex_retirement: CodexAppServerRetirementState,
}

impl NonAcpStreamSummary {
    /// Marks the start of a thinking block (including empty default-display anchor blocks).
    pub fn begin_thinking_block(&mut self) {
        self.thinking_block_open = true;
        self.current_thinking_text.clear();
    }

    /// Finalizes the active thinking block into [`ClaudeThinkingTracker`] if open (`#41085`).
    pub fn finish_thinking_block(&mut self, message_id: &str) {
        if !self.thinking_block_open {
            return;
        }
        self.thinking_block_open = false;
        self.thinking_seq = self.thinking_seq.saturating_add(1);
        let item_id = format!("{message_id}-think-{}", self.thinking_seq);
        let completed = std::mem::take(&mut self.current_thinking_text);
        self.thinking_tracker
            .record_thinking(message_id, &item_id, &completed);
    }

    /// Parses Claude `stream_event` thinking block start/stop envelopes for `#41085`.
    pub fn handle_claude_thinking_line(&mut self, line: &str, assistant_message_id: &str) {
        if let Ok(val) = serde_json::from_str::<Value>(line)
            && val.get("type").and_then(Value::as_str) == Some("stream_event")
            && let Some(ev) = val.get("event")
        {
            match ev.get("type").and_then(Value::as_str) {
                Some("content_block_start")
                    if ev
                        .get("content_block")
                        .and_then(|b| b.get("type"))
                        .and_then(Value::as_str)
                        == Some("thinking") =>
                {
                    self.begin_thinking_block();
                }
                Some("content_block_stop") => {
                    self.finish_thinking_block(assistant_message_id);
                }
                _ => {}
            }
        }
    }
}

/// Apply one normalized [`NonAcpTurnUpdate`] to `ctx`, emitting orchestration domain events.
pub fn apply_non_acp_update(
    ctx: &mut TurnEmitContext,
    summary: &mut NonAcpStreamSummary,
    update: NonAcpTurnUpdate,
    now_iso: &str,
    model_name: &str,
) -> Result<Vec<DomainEvent>, ServerRuntimeError> {
    let mut events = Vec::new();
    match update {
        NonAcpTurnUpdate::SessionStarted {
            provider_session_id,
            model,
        } => {
            summary.thinking_tracker.clear();
            summary.current_thinking_text.clear();
            summary.thinking_block_open = false;
            summary.native_session_id = Some(provider_session_id);
            let effective_model = model.as_deref().unwrap_or(model_name);
            let session =
                ctx.build_provider_session("running", effective_model, &ctx.started_at, now_iso);
            events.push(ctx.wrap_event(
                now_iso,
                DomainEventPayload::ProviderSessionUpdated {
                    event_type: "provider-session.updated".to_owned(),
                    session,
                },
            )?);
        }
        NonAcpTurnUpdate::AssistantTextDelta { text } => {
            summary.finish_thinking_block(ctx.assistant_message_id.as_str());
            if !text.is_empty() {
                ctx.assistant_text.push_str(&text);
                events.extend(ctx.build_assistant_message_events(true, now_iso)?);
            }
        }
        NonAcpTurnUpdate::ReasoningDelta { text } => {
            if !summary.thinking_block_open {
                summary.begin_thinking_block();
            }
            summary.current_thinking_text.push_str(&text);
            if !text.is_empty() {
                ctx.reasoning_text.push_str(&text);
                events.push(ctx.build_reasoning_event(true, now_iso)?);
            }
        }
        NonAcpTurnUpdate::ToolCallStarted {
            call_id,
            name,
            input_json,
        } => {
            summary.finish_thinking_block(ctx.assistant_message_id.as_str());
            if let Some(pre_tool) = summary
                .thinking_tracker
                .take_pre_tool_update(model_name, ctx.assistant_message_id.as_str())
            {
                if !ctx.assistant_text.is_empty() {
                    ctx.assistant_text.push('\n');
                }
                ctx.assistant_text.push_str(pre_tool.trim());
                events.extend(ctx.build_assistant_message_events(true, now_iso)?);
            }
            push_tool_started(ctx, &mut events, call_id, name, input_json, now_iso)?;
        }
        NonAcpTurnUpdate::ToolCallCompleted {
            call_id,
            output_json,
            is_error,
        } => push_tool_completed(ctx, &mut events, &call_id, output_json, is_error, now_iso)?,
        NonAcpTurnUpdate::PermissionRequested { .. } => {}
        NonAcpTurnUpdate::UsageReported {
            input_tokens,
            output_tokens,
        } => {
            summary.usage = Some((input_tokens, output_tokens));
            ctx.turn_token_usage = Some(json!({
                "usageScope": "main_agent",
                "hasSubagents": false,
                "usageStatus": "complete",
                "inputTokens": input_tokens,
                "outputTokens": output_tokens,
            }));
            let turn = ctx.build_provider_turn(
                "running",
                summary.run_ordinal.max(1),
                Some(&ctx.started_at),
                None,
            );
            events.push(ctx.wrap_event(now_iso, DomainEventPayload::ProviderTurnUpdated(turn))?);
        }
        NonAcpTurnUpdate::TurnCompleted { .. } => {
            summary.finish_thinking_block(ctx.assistant_message_id.as_str());
            summary.thinking_tracker.clear();
            summary.terminal_seen = true;
        }
        NonAcpTurnUpdate::TurnFailed { message } => {
            summary.finish_thinking_block(ctx.assistant_message_id.as_str());
            summary.thinking_tracker.clear();
            if ctx.driver.as_str() == "codex" {
                summary.codex_retirement.mark_terminated(&message);
            }
            summary.terminal_seen = true;
            summary.failure_message = Some(message);
        }
    }
    Ok(events)
}

fn push_tool_started(
    ctx: &mut TurnEmitContext,
    events: &mut Vec<DomainEvent>,
    call_id: String,
    name: String,
    input_json: Value,
    now_iso: &str,
) -> Result<(), ServerRuntimeError> {
    let ord = ctx.alloc_ordinal();
    let item_id = TurnItemId::new(format!("item-tool-{}-{}", ctx.run_id.as_str(), ord))
        .map_err(|e| ServerRuntimeError::Persistence(e.to_string()))?;
    ctx.tool_inputs.insert(call_id.clone(), input_json.clone());
    ctx.tool_items
        .insert(call_id, (item_id.clone(), name.clone(), ord));
    let title = name.clone();
    let item = ctx.make_turn_item(
        TurnItemMeta {
            id: item_id,
            ordinal: ord,
            status: TurnItemStatus::Running,
            completed_at: None,
            title: Some(&title),
            now_iso,
        },
        TurnItemKind::DynamicTool {
            tool_name: Some(name),
            viewed_image_path: None,
            input: input_json,
            output: None,
            output_omitted: None,
        },
    );
    events.push(ctx.wrap_event(now_iso, DomainEventPayload::TurnItemUpdated(Box::new(item)))?);
    Ok(())
}

fn push_tool_completed(
    ctx: &mut TurnEmitContext,
    events: &mut Vec<DomainEvent>,
    call_id: &str,
    output_json: Value,
    is_error: bool,
    now_iso: &str,
) -> Result<(), ServerRuntimeError> {
    let (item_id, name, ord) = if let Some(entry) = ctx.tool_items.get(call_id) {
        entry.clone()
    } else {
        let ord = ctx.alloc_ordinal();
        let id = TurnItemId::new(format!("item-tool-{}-{}", ctx.run_id.as_str(), ord))
            .map_err(|e| ServerRuntimeError::Persistence(e.to_string()))?;
        ctx.tool_items
            .insert(call_id.to_owned(), (id.clone(), call_id.to_owned(), ord));
        (id, call_id.to_owned(), ord)
    };
    let status = if is_error {
        TurnItemStatus::Failed
    } else {
        TurnItemStatus::Completed
    };
    let input = ctx.tool_inputs.get(call_id).cloned().unwrap_or(Value::Null);
    let title = name.clone();
    let item = ctx.make_turn_item(
        TurnItemMeta {
            id: item_id,
            ordinal: ord,
            status,
            completed_at: Some(now_iso),
            title: Some(&title),
            now_iso,
        },
        TurnItemKind::DynamicTool {
            tool_name: Some(name),
            viewed_image_path: None,
            input,
            output: Some(output_json),
            output_omitted: None,
        },
    );
    events.push(ctx.wrap_event(now_iso, DomainEventPayload::TurnItemUpdated(Box::new(item)))?);
    Ok(())
}

pub(crate) fn commit_http_turn_finish(
    sink: &legion_orchestrator::sink::EventSink,
    input: &crate::non_acp_http::NonAcpHttpTurnInput,
    ctx: &mut TurnEmitContext,
    summary: &NonAcpStreamSummary,
    stream_res: Result<(), ServerRuntimeError>,
) -> Result<(), ServerRuntimeError> {
    let (final_status, err_note) = if stream_res.is_ok() && summary.failure_message.is_none() {
        (legion_contracts::orchestration::RunStatus::Completed, None)
    } else {
        let msg = stream_res
            .as_ref()
            .err()
            .map(ToString::to_string)
            .or_else(|| summary.failure_message.clone());
        (legion_contracts::orchestration::RunStatus::Failed, msg)
    };
    let finish_events = ctx.build_finish_events_with_error(
        &input.run,
        input.attempt.as_ref(),
        final_status,
        err_note.as_deref(),
        &input.now_iso,
    )?;
    let mut followups = Vec::new();
    if final_status == legion_contracts::orchestration::RunStatus::Completed {
        let cmd_id =
            legion_contracts::ids::CommandId::new(format!("cmd-followup-{}", uuid::Uuid::new_v4()))
                .map_err(|e| ServerRuntimeError::Persistence(e.to_string()))?;
        followups.push(legion_orchestrator::outbox::PendingOutboxEffect {
            id: format!("eff-cp-{}", uuid::Uuid::new_v4()),
            command_id: cmd_id.clone(),
            thread_id: input.run.thread_id.clone(),
            request: legion_orchestrator::outbox::OutboxEffectRequest::CheckpointCapture {
                run_id: input.run.id.clone(),
                scope_id: input.scope_id.clone(),
            },
            available_at: None,
        });
        if input.queue_title_generate {
            followups.push(legion_orchestrator::outbox::PendingOutboxEffect {
                id: format!("eff-title-{}", uuid::Uuid::new_v4()),
                command_id: cmd_id,
                thread_id: input.run.thread_id.clone(),
                request: legion_orchestrator::outbox::OutboxEffectRequest::ThreadTitleGenerate {
                    kind: "initial".to_owned(),
                    message_id: Some(input.run.user_message_id.clone()),
                },
                available_at: None,
            });
        }
    }
    crate::turn_events::commit_runtime_events(
        sink,
        &input.run.thread_id,
        "runtime.http-provider.turn-finished",
        &input.now_iso,
        finish_events,
        followups,
    )?;
    stream_res?;
    if let Some(ref err_msg) = summary.failure_message {
        return Err(ServerRuntimeError::Upstream(err_msg.clone()));
    }
    Ok(())
}

/// Decides whether switching models should launch an updated Claude binary instead of reusing an
/// in-flight query (`#41086`).
#[must_use]
pub fn check_claude_model_switch_restart(input: &ClaudeModelSwitchRestartCheck<'_>) -> bool {
    should_start_updated_claude_on_model_switch(input)
}

/// Computes lost Claude background task reconciliation outcomes across process relaunch.
#[must_use]
pub fn plan_claude_lost_background_work<S: ::std::hash::BuildHasher>(
    exited_unexpectedly: bool,
    wake_requested: bool,
    buffered_task_notification_ids: &std::collections::HashSet<String, S>,
    awaited_roster_tasks: &[ClaudeTrackedBackgroundTask],
    owed_wake_tasks: &[ClaudeTrackedBackgroundTask],
) -> ClaudeLostBackgroundWorkPlan {
    resolve_claude_lost_background_work(
        exited_unexpectedly,
        wake_requested,
        buffered_task_notification_ids,
        awaited_roster_tasks,
        owed_wake_tasks,
    )
}
