//! Typed run-output schema validation, submission (`run.output.submit`), and `untilOutput`
//! settlement checks (`2770bf3c61` / #38874, `1f8f3698b0` / #40137).

use serde_json::Value;
use t3_contracts::orchestration::{
    DomainEvent, DomainEventPayload, Run, RunOutputPayload, RunOutputSchema, RunOutputSubmitted,
    RunStatus, SubmitRunOutputCommand, validate_run_output_schema_size,
};

pub use super::output_schema::validate_output_schema_value;
use super::output_schema::validate_schema_shape;
use crate::decider::thread_ops::{is_run_active, require_live_thread};
use crate::decider::types::{CommandDecision, DeciderError, derived_event_id};
use crate::sink::projection_apply::DeciderThreadState;

/// Returns `true` when `text` starts with a `/goal` provider command with no attachments.
#[must_use]
pub fn is_goal_command(text: &str, attachments: &[Value]) -> bool {
    if !attachments.is_empty() {
        return false;
    }
    let trimmed = text.trim();
    trimmed.eq_ignore_ascii_case("/goal")
        || (trimmed.len() > 5
            && trimmed[..5].eq_ignore_ascii_case("/goal")
            && trimmed.as_bytes()[5].is_ascii_whitespace())
}

/// Returns `true` when `message` is a provider command (`/compact`, `/logout`, `/goal ...`)
/// rather than a normal prompt, prohibiting structured `outputSchema` requests.
#[must_use]
pub fn is_provider_command(text: &str, attachments: &[Value]) -> bool {
    if !attachments.is_empty() {
        return false;
    }
    let lower = text.trim().to_ascii_lowercase();
    lower == "/compact" || lower == "/logout" || is_goal_command(text, attachments)
}

/// Validates a message + optional `outputSchema` request before creating or editing a run.
pub fn validate_output_request_for_message(
    text: &str,
    attachments: &[Value],
    schema: Option<&RunOutputSchema>,
) -> Result<(), DeciderError> {
    let Some(schema) = schema else {
        return Ok(());
    };
    if is_provider_command(text, attachments) {
        return Err(DeciderError::RunOutputRejected(
            "A command cannot be asked for a result. Ask for it in a message.".to_owned(),
        ));
    }
    validate_run_output_schema_size(schema)
        .map_err(|err| DeciderError::RunOutputRejected(err.to_string()))?;
    validate_schema_shape(schema, 0).map_err(DeciderError::RunOutputRejected)
}

/// Pure decision helper validating and committing a `run.output.submit` command (`#38874`).
pub fn decide_submit_run_output(
    state: &DeciderThreadState,
    cmd: &SubmitRunOutputCommand,
    now_iso: &str,
) -> Result<CommandDecision, DeciderError> {
    let thread = require_live_thread(state, &cmd.thread_id)?;
    let Some(run) = state.runs.iter().find(|r| r.id == cmd.run_id) else {
        return Err(DeciderError::RunOutputRejected(format!(
            "Run `{}` was not found on thread `{}`.",
            cmd.run_id, cmd.thread_id
        )));
    };
    let Some(schema) = &run.output_schema else {
        return Err(DeciderError::RunOutputRejected(format!(
            "Run `{}` was not asked for a result.",
            cmd.run_id
        )));
    };
    let owes_output = run.status == RunStatus::Completed && run.output.is_none();
    if !is_run_active(run) && !owes_output {
        return Err(DeciderError::RunOutputRejected(format!(
            "Run `{}` is stopped or already answered and takes no result.",
            cmd.run_id
        )));
    }
    validate_output_schema_value(schema, &cmd.value).map_err(|msg| {
        DeciderError::RunOutputRejected(format!("The result does not match the schema: {msg}"))
    })?;

    let payload = RunOutputSubmitted {
        run_id: cmd.run_id.clone(),
        output: RunOutputPayload {
            value: cmd.value.clone(),
            submitted_at: now_iso.to_owned(),
        },
    };
    let event = DomainEvent {
        id: derived_event_id(&cmd.command_id, 0)?,
        thread_id: cmd.thread_id.clone(),
        run_id: Some(cmd.run_id.clone()),
        node_id: None,
        driver: None,
        provider_instance_id: Some(run.provider_instance_id.clone()),
        raw_event_id: None,
        occurred_at: now_iso.to_owned(),
        payload: DomainEventPayload::RunOutputSubmitted(payload),
    };
    Ok(CommandDecision {
        command_id: cmd.command_id.clone(),
        command_type: "run.output.submit".to_owned(),
        thread_id: cmd.thread_id.clone(),
        events: vec![event],
        outbox_effects: Vec::new(),
        rollback_intent: None,
        resulting_thread: Some(thread),
    })
}

/// Resolves which run on a thread owes output during a live turn (`#38874`).
pub fn pick_run_for_output_submission(runs: &[Run]) -> Result<&Run, DeciderError> {
    let active = runs.iter().rev().find(|r| is_run_active(r));
    let Some(active_run) = active else {
        return Err(DeciderError::RunOutputRejected(
            "This turn was not asked for a result. Reply in your message instead.".to_owned(),
        ));
    };
    if active_run.output_schema.is_some() {
        return Ok(active_run);
    }
    let owed = runs
        .iter()
        .filter(|r| {
            r.output_schema.is_some() && r.output.is_none() && r.status == RunStatus::Completed
        })
        .max_by_key(|r| r.ordinal);
    owed.ok_or_else(|| {
        DeciderError::RunOutputRejected(
            "This turn was not asked for a result. Reply in your message instead.".to_owned(),
        )
    })
}

/// Resolution status for `waitForThread(..., untilOutput: true)` (`#40137`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UntilOutputStatus {
    /// Structured output was submitted and stored on `run.output`.
    ResolvedWithOutput,
    /// Run never declared `outputSchema` and has finished execution (`#40137`).
    ResolvedWithoutSchema,
    /// Run requested `outputSchema` but ended in failure/cancellation/interruption without output.
    TerminalWithoutOutput,
    /// Waiter must stay parked until output arrives or the run fails.
    Pending,
}

/// Evaluates whether a wait caller with `until_output = true` should resolve (`#38874`, `#40137`).
#[must_use]
pub fn evaluate_until_output_wait(run: &Run) -> UntilOutputStatus {
    if run.output.is_some() {
        return UntilOutputStatus::ResolvedWithOutput;
    }
    if run.output_schema.is_none() {
        return if is_run_active(run) {
            UntilOutputStatus::Pending
        } else {
            UntilOutputStatus::ResolvedWithoutSchema
        };
    }
    if matches!(
        run.status,
        RunStatus::Failed | RunStatus::Interrupted | RunStatus::Cancelled | RunStatus::RolledBack
    ) {
        return UntilOutputStatus::TerminalWithoutOutput;
    }
    UntilOutputStatus::Pending
}
