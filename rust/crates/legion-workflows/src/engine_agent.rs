#![forbid(unsafe_code)]
//! Durable agent step state machine (`agent` and `agent.try`) with corrections, timeouts, and quiet windows.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::engine_agent_turns::{
    is_run_cancelled, journaled_interrupt, journaled_result, journaled_start, record_done,
    thread_of,
};
use crate::journal::{JournalEntryStatus, WorkflowJournal};
use crate::schema::validate_json_schema_value;
use crate::store::WorkflowStore;
use crate::traits::{
    AgentEnding, AgentLaunch, AgentModel, IssueAddress, WorkflowAgentDriver, WorkflowForgeDriver,
};
use crate::types::{WorkflowError, WorkflowStepStatus};

const MAX_CORRECTIONS: usize = 2;

/// Thread reference returned by a completed `agent` step for continuation by later steps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentThreadRef {
    /// Thread ID.
    pub thread_id: String,
    /// Optional linked environment ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
}

/// Options configuring an `agent` or `agent.try` workflow step.
#[derive(Debug, Clone)]
pub struct AgentStepOptions {
    /// Prompt handed to the agent.
    pub brief: String,
    /// Optional custom thread title (defaults to `<workflow>: <step>`).
    pub title: Option<String>,
    /// Optional structured output JSON Schema.
    pub returns_schema: Option<Map<String, Value>>,
    /// Explicit model selection override.
    pub model: Option<AgentModel>,
    /// Project ID or workspace path for new threads.
    pub project: Option<String>,
    /// Linked environment ID for remote threads.
    pub environment: Option<String>,
    /// Existing thread to continue instead of launching a new one.
    pub thread: Option<AgentThreadRef>,
    /// Issue linked to a local agent thread.
    pub issue: Option<IssueAddress>,
    /// Launch options for new threads.
    pub launch: Option<AgentLaunch>,
    /// Optional step timeout in milliseconds.
    pub timeout_ms: Option<i64>,
    /// Optional quiet window in milliseconds when awaiting `outputSchema` across woken turns.
    pub quiet_ms: Option<i64>,
}

/// Outcome of `agent` or `agent.try`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "lowercase")]
pub enum AgentStepAttempt {
    /// Agent completed (and submitted output matching `returns_schema` if declared).
    Done {
        /// Output JSON value (or reply string when no `returns_schema` was given).
        value: Value,
        /// Thread used by the step.
        thread: AgentThreadRef,
    },
    /// Agent failed (only returned to workflow body when called via `agent.try`).
    Failed {
        /// Human-readable failure explanation.
        reason: String,
        /// Thread started before failure, if any.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread: Option<AgentThreadRef>,
    },
}

/// Outcome returned from [`execute_agent_step`] to replay logic.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentStepOutcome {
    /// Step reached a final `Done` or (`.try`) `Failed` value cached in journal.
    Finished(AgentStepAttempt),
    /// Run paused or cancelled; replay must suspend now.
    Suspended,
}

/// Borrowed execution dependencies for [`execute_agent_step`].
pub struct AgentStepDeps<'a> {
    pub store: &'a WorkflowStore,
    pub journal: &'a WorkflowJournal,
    pub agents: &'a Arc<dyn WorkflowAgentDriver>,
    pub forge: &'a Arc<dyn WorkflowForgeDriver>,
    pub run_id: &'a str,
    pub workflow: &'a str,
    pub step_name: &'a str,
    pub pause_on_failure: bool,
}

/// Executes or replays an `agent` / `agent.try` step deterministically against `journal`.
pub async fn execute_agent_step(
    deps: &AgentStepDeps<'_>,
    opts: &AgentStepOptions,
) -> Result<AgentStepOutcome, WorkflowError> {
    if opts.thread.is_some()
        && (opts.project.is_some()
            || opts.environment.is_some()
            || opts.model.is_some()
            || opts.title.is_some()
            || opts.launch.is_some())
    {
        return Err(WorkflowError::Input {
            problem: format!(
                "agent({:?}) continues `thread`, which keeps how it started. Drop `project`/`environment`/`model`/`title`/`launch`.",
                deps.step_name
            ),
        });
    }
    let summary_key = format!("agent/{}/summary", deps.step_name);
    if let Some(entry) = deps.journal.get_entry(deps.run_id, &summary_key)?
        && entry.status == JournalEntryStatus::Completed
        && let Some(raw) = entry.payload_json
        && let Ok(attempt) = serde_json::from_str::<AgentStepAttempt>(&raw)
    {
        return Ok(AgentStepOutcome::Finished(attempt));
    }

    let quiet_ms = opts.returns_schema.as_ref().and(opts.quiet_ms);
    for round in 0_usize.. {
        if let Some(outcome) = run_agent_round(deps, opts, round, quiet_ms, &summary_key).await? {
            return Ok(outcome);
        }
    }
    unreachable!("unbounded round loop only exits via return")
}

async fn run_agent_round(
    deps: &AgentStepDeps<'_>,
    opts: &AgentStepOptions,
    round: usize,
    quiet_ms: Option<i64>,
    summary_key: &str,
) -> Result<Option<AgentStepOutcome>, WorkflowError> {
    if is_run_cancelled(deps.store, deps.run_id)? {
        return Ok(Some(AgentStepOutcome::Suspended));
    }
    let mut turn = journaled_start(
        deps,
        opts,
        round,
        &opts.brief,
        opts.thread.as_ref(),
        "start",
    )
    .await?;
    let started_at = turn.started_at;
    let mut problem = match &turn.failed {
        Some(err) => format!("the agent could not be started: {err}"),
        None => String::new(),
    };
    let mut died = false;

    for correction in 0..=MAX_CORRECTIONS {
        if turn.failed.is_some() {
            break;
        }
        let result =
            journaled_result(deps, opts, round, correction, &turn, started_at, quiet_ms).await?;
        match evaluate_turn_result(deps, opts, round, correction, &turn, &result, summary_key)
            .await?
        {
            TurnEvalOutcome::Finished(outcome) => return Ok(Some(outcome)),
            TurnEvalOutcome::Died(msg) => {
                died = true;
                problem = msg;
                break;
            }
            TurnEvalOutcome::StopLoop(msg) => {
                problem = msg;
                break;
            }
            TurnEvalOutcome::RetryWithTurn(corrected, msg) => {
                problem = msg;
                turn = corrected;
            }
        }
    }

    let mut lingers = false;
    if died && turn.failed.is_none() && !journaled_interrupt(deps, round, &turn).await? {
        lingers = true;
        problem = format!("{problem}; its turn could not be stopped");
    }
    if round.is_multiple_of(2) && died && !lingers {
        return Ok(None);
    }
    if !deps.pause_on_failure {
        deps.store.step_ended(
            deps.run_id,
            deps.step_name,
            WorkflowStepStatus::Failed,
            Some(&problem),
        )?;
        deps.store.run_resumed(deps.run_id)?;
        let attempt = AgentStepAttempt::Failed {
            reason: problem,
            thread: turn.failed.is_none().then(|| thread_of(&turn)),
        };
        let val = serde_json::to_value(&attempt).map_err(|e| WorkflowError::Storage {
            message: e.to_string(),
        })?;
        deps.journal
            .record_activity_success(deps.run_id, summary_key, &val)?;
        return Ok(Some(AgentStepOutcome::Finished(attempt)));
    }

    let pause_key = format!("agent/{}/{round}/pause", deps.step_name);
    if let Some(entry) = deps.journal.get_entry(deps.run_id, &pause_key)?
        && entry.status == JournalEntryStatus::Completed
    {
        return Ok(None);
    }
    let mark_name = format!("{}/pause/{round}", deps.step_name);
    if deps.store.mark(deps.run_id, &mark_name)? > 1 {
        deps.journal
            .record_activity_success(deps.run_id, &pause_key, &Value::Null)?;
        return Ok(None);
    }
    deps.store.step_ended(
        deps.run_id,
        deps.step_name,
        WorkflowStepStatus::Failed,
        Some(&problem),
    )?;
    let pause_msg = format!("{}: {problem}", deps.step_name);
    deps.store.run_paused(deps.run_id, &pause_msg, Some(true))?;
    deps.journal
        .record_activity_failure(deps.run_id, &pause_key, &pause_msg)?;
    Ok(Some(AgentStepOutcome::Suspended))
}

enum TurnEvalOutcome {
    Finished(AgentStepOutcome),
    Died(String),
    StopLoop(String),
    RetryWithTurn(crate::traits::AgentTurn, String),
}

async fn evaluate_turn_result(
    deps: &AgentStepDeps<'_>,
    opts: &AgentStepOptions,
    round: usize,
    correction: usize,
    turn: &crate::traits::AgentTurn,
    result: &crate::traits::AgentResult,
    summary_key: &str,
) -> Result<TurnEvalOutcome, WorkflowError> {
    match result.ending {
        AgentEnding::Stopped => {
            return Ok(TurnEvalOutcome::StopLoop(format!(
                "the agent's turn was stopped: {}",
                result.detail
            )));
        }
        AgentEnding::Failed => {
            return Ok(TurnEvalOutcome::Died(format!(
                "the agent's turn was cut short: {}",
                result.detail
            )));
        }
        AgentEnding::Completed => {}
    }
    let thread = thread_of(turn);
    let Some(ref schema) = opts.returns_schema else {
        let value = Value::String(result.reply.clone());
        record_done(deps, summary_key, &value, None, &thread)?;
        return Ok(TurnEvalOutcome::Finished(AgentStepOutcome::Finished(
            AgentStepAttempt::Done { value, thread },
        )));
    };
    let misfit = match &result.output {
        None => None,
        Some(out) => match validate_json_schema_value(schema, out, true) {
            Ok(()) => {
                record_done(deps, summary_key, out, Some(&result.reply), &thread)?;
                return Ok(TurnEvalOutcome::Finished(AgentStepOutcome::Finished(
                    AgentStepAttempt::Done {
                        value: out.clone(),
                        thread,
                    },
                )));
            }
            Err(err) => Some(err),
        },
    };
    let problem = match &misfit {
        None => "the agent did not return a result: its turn ended with none accepted".to_owned(),
        Some(err) => format!("the agent's result did not fit: {err}"),
    };
    if correction >= MAX_CORRECTIONS {
        return Ok(TurnEvalOutcome::StopLoop(problem));
    }
    let prompt = match &misfit {
        None => "Your turn ended without an accepted result. Do not redo the work: call `legion_submit_output` now with the result of what you did.".to_owned(),
        Some(err) => format!("The result you submitted does not fit: {err}\nDo not redo the work: call `legion_submit_output` again with a corrected result."),
    };
    let subkey = format!("correct/{correction}");
    let corrected = journaled_start(deps, opts, round, &prompt, Some(&thread), &subkey).await?;
    if let Some(err) = &corrected.failed {
        return Ok(TurnEvalOutcome::StopLoop(format!(
            "{problem}, and it could not be asked again: {err}"
        )));
    }
    Ok(TurnEvalOutcome::RetryWithTurn(corrected, problem))
}
