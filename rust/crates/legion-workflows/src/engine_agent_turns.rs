#![forbid(unsafe_code)]
//! Journaled agent turn start, result collection, timeout interruption, and recording helpers.

use std::sync::Arc;

use serde_json::{Map, Value};

use crate::engine_agent::{AgentStepAttempt, AgentStepDeps, AgentStepOptions, AgentThreadRef};
use crate::journal::JournalEntryStatus;
use crate::store::WorkflowStore;
use crate::time::{format_duration_human, now_epoch_ms};
use crate::traits::{
    AgentEnding, AgentInterruptOutcome, AgentInterruptTarget, AgentOrigin, AgentResult,
    AgentStartInput, AgentTurn, WorkflowAgentDriver, WorkflowAgentError,
};
use crate::types::{
    WorkflowError, WorkflowReferenceInput, WorkflowRunStatus, WorkflowStepKind, WorkflowStepStatus,
};

pub(crate) async fn journaled_start(
    deps: &AgentStepDeps<'_>,
    opts: &AgentStepOptions,
    round: usize,
    prompt: &str,
    thread: Option<&AgentThreadRef>,
    subkey: &str,
) -> Result<AgentTurn, WorkflowError> {
    let entry_key = format!("agent/{}/{round}/{subkey}", deps.step_name);
    if let Some(entry) = deps.journal.get_entry(deps.run_id, &entry_key)?
        && entry.status == JournalEntryStatus::Completed
        && let Some(raw) = entry.payload_json
        && let Ok(turn) = serde_json::from_str::<AgentTurn>(&raw)
    {
        return Ok(turn);
    }
    if subkey == "start" {
        deps.store
            .step_started(deps.run_id, deps.step_name, WorkflowStepKind::Agent)?;
    }
    let is_original_thread = thread.is_some() && opts.thread.as_ref() == thread;
    let start_input = AgentStartInput {
        key: format!("{}/{}/{round}/{subkey}", deps.run_id, deps.step_name),
        origin: AgentOrigin {
            run_id: deps.run_id.to_owned(),
            workflow: deps.workflow.to_owned(),
            step: deps.step_name.to_owned(),
        },
        title: opts
            .title
            .clone()
            .unwrap_or_else(|| format!("{}: {}", deps.workflow, deps.step_name)),
        prompt: prompt.to_owned(),
        output_schema: opts.returns_schema.clone(),
        model: if thread.is_none() {
            opts.model.clone()
        } else {
            None
        },
        project: if thread.is_none() {
            opts.project.clone()
        } else {
            None
        },
        environment: thread.map_or_else(|| opts.environment.clone(), |t| t.environment.clone()),
        thread: thread.map(|t| t.thread_id.clone()),
        issue: opts.issue.clone(),
        launch: if thread.is_none() || is_original_thread {
            opts.launch.clone()
        } else {
            None
        },
        stable_launch_key: opts
            .launch
            .as_ref()
            .and_then(|l| l.stable_launch_key.clone()),
    };
    if let Some(ref issue_addr) = opts.issue
        && let Ok(detail) = deps.forge.issues_read(issue_addr.clone()).await
        && let Some(ref_input) = issue_ref_from_detail(&detail)
    {
        let _ = deps
            .store
            .reference_noted(deps.run_id, Some(deps.step_name), &ref_input);
    }
    let turn = match deps.agents.start(start_input).await {
        Ok(mut started) => {
            if started.started_at.is_none() {
                started.started_at = Some(now_epoch_ms());
            }
            deps.store.step_thread(
                deps.run_id,
                deps.step_name,
                &started.thread_id,
                started.environment.as_deref(),
                &started.turn_id,
            )?;
            started
        }
        Err(err) => {
            if deps.pause_on_failure {
                return Err(WorkflowError::Input {
                    problem: err.reason,
                });
            }
            AgentTurn {
                thread_id: String::new(),
                turn_id: String::new(),
                environment: None,
                started_at: Some(now_epoch_ms()),
                failed: Some(err.reason),
            }
        }
    };
    let val = serde_json::to_value(&turn).map_err(|e| WorkflowError::Storage {
        message: e.to_string(),
    })?;
    deps.journal
        .record_activity_success(deps.run_id, &entry_key, &val)?;
    Ok(turn)
}

pub(crate) async fn journaled_result(
    deps: &AgentStepDeps<'_>,
    opts: &AgentStepOptions,
    round: usize,
    correction: usize,
    turn: &AgentTurn,
    started_at: Option<i64>,
    quiet_ms: Option<i64>,
) -> Result<AgentResult, WorkflowError> {
    let entry_key = format!("agent/{}/{round}/result/{correction}", deps.step_name);
    if let Some(entry) = deps.journal.get_entry(deps.run_id, &entry_key)?
        && entry.status == JournalEntryStatus::Completed
        && let Some(raw) = entry.payload_json
        && let Ok(res) = serde_json::from_str::<AgentResult>(&raw)
    {
        return Ok(res);
    }
    let wait_detail = format!("agent {}", deps.step_name);
    deps.store
        .run_status(deps.run_id, WorkflowRunStatus::Waiting, Some(&wait_detail))?;
    let result = wait_turn_within_timeout(deps, opts, turn, started_at, quiet_ms).await?;
    let val = serde_json::to_value(&result).map_err(|e| WorkflowError::Storage {
        message: e.to_string(),
    })?;
    deps.journal
        .record_activity_success(deps.run_id, &entry_key, &val)?;
    Ok(result)
}

async fn wait_turn_within_timeout(
    deps: &AgentStepDeps<'_>,
    opts: &AgentStepOptions,
    turn: &AgentTurn,
    started_at: Option<i64>,
    quiet_ms: Option<i64>,
) -> Result<AgentResult, WorkflowError> {
    let fut = deps.agents.result(turn.clone(), quiet_ms);
    let Some(limit_ms) = opts.timeout_ms else {
        return map_agent_result(deps, fut.await);
    };
    let elapsed = started_at.map_or(0, |s| now_epoch_ms().saturating_sub(s));
    let left_ms = u64::try_from(limit_ms.saturating_sub(elapsed).max(0)).unwrap_or(0);
    if let Ok(inner) = tokio::time::timeout(std::time::Duration::from_millis(left_ms), fut).await {
        return map_agent_result(deps, inner);
    }
    let stop_res = interrupt_with_retries(deps.agents, turn).await;
    let late = format!("it was not done after {}", format_duration_human(limit_ms));
    match stop_res {
        Ok(AgentInterruptOutcome::Ended) => {
            let ended = map_agent_result(deps, deps.agents.result(turn.clone(), None).await)?;
            if ended.ending != AgentEnding::Completed
                || opts.returns_schema.is_none()
                || ended.output.is_some()
            {
                return Ok(ended);
            }
            Ok(AgentResult {
                ending: AgentEnding::Stopped,
                detail: format!(
                    "{late}: its turn ended without its result, and the agent may still be at work on its thread"
                ),
                reply: String::new(),
                output: None,
            })
        }
        Ok(AgentInterruptOutcome::Stopped) => Ok(AgentResult {
            ending: AgentEnding::Stopped,
            detail: late,
            reply: String::new(),
            output: None,
        }),
        Err(err) => {
            if deps.pause_on_failure {
                return Err(WorkflowError::Input {
                    problem: err.reason,
                });
            }
            Ok(AgentResult {
                ending: AgentEnding::Stopped,
                detail: format!("{late}, and its turn could not be stopped"),
                reply: String::new(),
                output: None,
            })
        }
    }
}

pub(crate) async fn journaled_interrupt(
    deps: &AgentStepDeps<'_>,
    round: usize,
    turn: &AgentTurn,
) -> Result<bool, WorkflowError> {
    let entry_key = format!("agent/{}/{round}/stop", deps.step_name);
    if let Some(entry) = deps.journal.get_entry(deps.run_id, &entry_key)?
        && entry.status == JournalEntryStatus::Completed
        && let Some(raw) = entry.payload_json
        && let Ok(val) = serde_json::from_str::<bool>(&raw)
    {
        return Ok(val);
    }
    let stopped = match interrupt_with_retries(deps.agents, turn).await {
        Ok(_) => true,
        Err(err) => {
            if deps.pause_on_failure {
                return Err(WorkflowError::Input {
                    problem: err.reason,
                });
            }
            false
        }
    };
    deps.journal
        .record_activity_success(deps.run_id, &entry_key, &Value::Bool(stopped))?;
    Ok(stopped)
}

async fn interrupt_with_retries(
    agents: &Arc<dyn WorkflowAgentDriver>,
    turn: &AgentTurn,
) -> Result<AgentInterruptOutcome, WorkflowAgentError> {
    let target = AgentInterruptTarget {
        thread_id: turn.thread_id.clone(),
        environment: turn.environment.clone(),
        turn_id: Some(turn.turn_id.clone()),
    };
    let mut last_err = None;
    for _ in 0..3 {
        match agents.interrupt(target.clone()).await {
            Ok(outcome) => return Ok(outcome),
            Err(err) => last_err = Some(err),
        }
    }
    Err(last_err.unwrap_or_else(|| WorkflowAgentError::new("interrupt failed")))
}

fn map_agent_result(
    deps: &AgentStepDeps<'_>,
    res: Result<AgentResult, WorkflowAgentError>,
) -> Result<AgentResult, WorkflowError> {
    match res {
        Ok(r) => Ok(r),
        Err(err) => {
            if deps.pause_on_failure {
                Err(WorkflowError::Input {
                    problem: err.reason,
                })
            } else {
                Ok(AgentResult {
                    ending: AgentEnding::Failed,
                    detail: err.reason,
                    reply: String::new(),
                    output: None,
                })
            }
        }
    }
}

pub(crate) fn record_done(
    deps: &AgentStepDeps<'_>,
    summary_key: &str,
    produced: &Value,
    said: Option<&str>,
    thread: &AgentThreadRef,
) -> Result<(), WorkflowError> {
    deps.store.step_ended(
        deps.run_id,
        deps.step_name,
        WorkflowStepStatus::Completed,
        None,
    )?;
    deps.store
        .step_result(deps.run_id, deps.step_name, produced, said)?;
    deps.store.run_resumed(deps.run_id)?;
    let attempt = AgentStepAttempt::Done {
        value: produced.clone(),
        thread: thread.clone(),
    };
    let val = serde_json::to_value(&attempt).map_err(|e| WorkflowError::Storage {
        message: e.to_string(),
    })?;
    deps.journal
        .record_activity_success(deps.run_id, summary_key, &val)?;
    Ok(())
}

pub(crate) fn thread_of(turn: &AgentTurn) -> AgentThreadRef {
    AgentThreadRef {
        thread_id: turn.thread_id.clone(),
        environment: turn.environment.clone(),
    }
}

pub(crate) fn is_run_cancelled(store: &WorkflowStore, run_id: &str) -> Result<bool, WorkflowError> {
    Ok(store
        .run(run_id)?
        .is_some_and(|r| r.status == WorkflowRunStatus::Cancelled))
}

fn issue_ref_from_detail(detail: &Value) -> Option<WorkflowReferenceInput> {
    let obj = detail.as_object()?;
    let project_id = obj.get("projectId")?.as_str()?;
    let tracker = obj.get("tracker")?.as_str()?;
    let scope = obj.get("scope")?.as_str()?;
    let key = obj.get("key")?.as_str()?;
    let host = obj.get("host").and_then(Value::as_str);
    let title = obj.get("title").and_then(Value::as_str).unwrap_or("");
    let url = obj.get("url").and_then(Value::as_str).map(str::to_owned);
    let state = obj
        .get("state")
        .and_then(|s| {
            s.get("category")
                .and_then(Value::as_str)
                .or_else(|| s.as_str())
        })
        .map(str::to_owned);
    let mut addr = Map::new();
    addr.insert("projectId".to_owned(), Value::String(project_id.to_owned()));
    addr.insert("tracker".to_owned(), Value::String(tracker.to_owned()));
    if let Some(h) = host {
        addr.insert("host".to_owned(), Value::String(h.to_owned()));
    }
    addr.insert("scope".to_owned(), Value::String(scope.to_owned()));
    addr.insert("key".to_owned(), Value::String(key.to_owned()));
    Some(WorkflowReferenceInput {
        kind: "issue".to_owned(),
        key: serde_json::json!([tracker, host.unwrap_or(""), scope.to_lowercase(), key])
            .to_string(),
        address: Value::Object(addr),
        title: title.to_owned(),
        url,
        state,
    })
}
