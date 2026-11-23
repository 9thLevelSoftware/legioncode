//! Replay-safe wait, ask, pull-request watch, and buffered mail delivery helpers.

use std::collections::HashSet;

use serde_json::{Map, Value};
use uuid::Uuid;

use crate::journal::{JournalEntryStatus, WorkflowJournal};
use crate::mail_deliver::MailDeliverOutcome;
use crate::schema::validate_json_schema_value;
use crate::store::WorkflowStore;
use crate::time::{format_millis_iso, now_epoch_ms};
use crate::traits::{PullRequestAddress, PullRequestUntil, PullRequestWatch};
use crate::types::{SignalAddress, WorkflowError, WorkflowStepKind, WorkflowStepStatus};
use crate::waits_mail::{NewWaitInput, WaitRow};

/// Runtime codec check applied after JSON Schema validation on a wait signal.
pub type WaitCodecFn = dyn Fn(&Value) -> Result<Value, String> + Send + Sync;

/// Parameters for opening or replaying a durable `wait` / `ask` / `pullRequests.wait`.
pub struct WaitOptions<'a> {
    /// Step kind (`Wait` or `Ask`).
    pub kind: WorkflowStepKind,
    /// Unique step name within the run.
    pub name: &'a str,
    /// Optional channel key scoped to the workflow.
    pub channel: Option<&'a str>,
    /// JSON Schema validating incoming signals.
    pub schema: &'a Map<String, Value>,
    /// Optional timeout duration in milliseconds.
    pub timeout_ms: Option<i64>,
    /// Optional question prompt JSON (`ask`).
    pub request: Option<&'a Value>,
    /// Optional pull-request watch descriptor.
    pub watch: Option<&'a PullRequestWatch>,
    /// Optional runtime value codec (e.g. `Schema.FiniteFromString` decode).
    pub codec: Option<&'a WaitCodecFn>,
}

/// Outcome of executing a durable wait on one replay pass.
#[derive(Debug, Clone, PartialEq)]
pub enum WaitReplayOutcome {
    /// Resolved with `Some(value)` on signal or `None` on timeout.
    Resolved(Option<Value>),
    /// Still open and awaiting a signal, deadline, or watch match.
    Waiting,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct JournalWaitRecord {
    resolved: bool,
    value: Value,
}

/// Executes or replays a durable wait/ask/PR-watch step against `store` and `journal`.
pub fn execute_wait_replay(
    store: &WorkflowStore,
    journal: &WorkflowJournal,
    run_id: &str,
    workflow: &str,
    opts: &WaitOptions<'_>,
) -> Result<WaitReplayOutcome, WorkflowError> {
    let journal_key = format!("wait/{}", opts.name);
    if let Some(entry) = journal.get_entry(run_id, &journal_key)?
        && entry.status == JournalEntryStatus::Completed
        && let Some(raw) = entry.payload_json
        && let Ok(rec) = serde_json::from_str::<JournalWaitRecord>(&raw)
    {
        return Ok(WaitReplayOutcome::Resolved(
            rec.resolved.then_some(rec.value),
        ));
    }

    let existing = store.wait_row(run_id, opts.name)?;
    if existing.is_none() {
        let token = Uuid::new_v4().to_string();
        let deadline = opts
            .timeout_ms
            .map(|ms| format_millis_iso(now_epoch_ms().saturating_add(ms.max(0))));
        let schema_json = serde_json::to_string(opts.schema).map_err(storage_err)?;
        let request_json = opts
            .request
            .map(serde_json::to_string)
            .transpose()
            .map_err(storage_err)?;
        let watch_json = opts
            .watch
            .map(serde_json::to_string)
            .transpose()
            .map_err(storage_err)?;
        store.step_started(run_id, opts.name, opts.kind)?;
        store.wait_opened(&NewWaitInput {
            run_id: run_id.to_owned(),
            name: opts.name.to_owned(),
            workflow: workflow.to_owned(),
            key: opts.channel.map(str::to_owned),
            token,
            schema_json,
            request_json,
            watch_json,
            deadline,
        })?;
        store.run_status(
            run_id,
            crate::types::WorkflowRunStatus::Waiting,
            Some(opts.name),
        )?;
        deliver_early_mail(store, run_id, workflow, opts)?;
    }

    let Some(wait_row) = store.wait_row(run_id, opts.name)? else {
        return Ok(WaitReplayOutcome::Waiting);
    };
    let Some(woke_raw) = wait_row.woke_json else {
        // Re-check early buffered mail in case mail arrived between open and check.
        deliver_early_mail(store, run_id, workflow, opts)?;
        let Some(refreshed) = store.wait_row(run_id, opts.name)? else {
            return Ok(WaitReplayOutcome::Waiting);
        };
        let Some(refreshed_woke) = refreshed.woke_json else {
            return Ok(WaitReplayOutcome::Waiting);
        };
        return finish_or_reopen_wait(
            store,
            journal,
            run_id,
            workflow,
            opts,
            &journal_key,
            &refreshed_woke,
        );
    };
    finish_or_reopen_wait(
        store,
        journal,
        run_id,
        workflow,
        opts,
        &journal_key,
        &woke_raw,
    )
}

fn finish_or_reopen_wait(
    store: &WorkflowStore,
    journal: &WorkflowJournal,
    run_id: &str,
    workflow: &str,
    opts: &WaitOptions<'_>,
    journal_key: &str,
    woke_raw: &str,
) -> Result<WaitReplayOutcome, WorkflowError> {
    let woke: Value = serde_json::from_str(woke_raw).map_err(storage_err)?;
    let expired = woke
        .get("expired")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if expired {
        store.wait_closed(run_id, opts.name)?;
        store.step_ended(
            run_id,
            opts.name,
            WorkflowStepStatus::Completed,
            Some("timed out"),
        )?;
        store.run_resumed(run_id)?;
        let rec = serde_json::json!({ "resolved": false, "value": Value::Null });
        journal.record_activity_success(run_id, journal_key, &rec)?;
        return Ok(WaitReplayOutcome::Resolved(None));
    }

    let raw_val = woke.get("value").cloned().unwrap_or(Value::Null);
    let checked = check_wait_value(opts, &raw_val);
    match checked {
        Err(reason) => {
            let problem =
                format!("It was signalled with a value it cannot read, and waits again: {reason}");
            let next_token = Uuid::new_v4().to_string();
            store.wait_reopened(run_id, opts.name, &next_token)?;
            let capped: String = problem.chars().take(2_000).collect();
            store.step_detail(run_id, opts.name, &capped)?;
            deliver_early_mail(store, run_id, workflow, opts)?;
            let Some(after_reopen) = store.wait_row(run_id, opts.name)? else {
                return Ok(WaitReplayOutcome::Waiting);
            };
            if let Some(next_woke) = after_reopen.woke_json {
                return finish_or_reopen_wait(
                    store,
                    journal,
                    run_id,
                    workflow,
                    opts,
                    journal_key,
                    &next_woke,
                );
            }
            Ok(WaitReplayOutcome::Waiting)
        }
        Ok(decoded) => {
            store.wait_closed(run_id, opts.name)?;
            store.step_ended(run_id, opts.name, WorkflowStepStatus::Completed, None)?;
            store.step_result(run_id, opts.name, &raw_val, None)?;
            store.run_resumed(run_id)?;
            let rec = serde_json::json!({ "resolved": true, "value": decoded });
            journal.record_activity_success(run_id, journal_key, &rec)?;
            Ok(WaitReplayOutcome::Resolved(Some(decoded)))
        }
    }
}

fn deliver_early_mail(
    store: &WorkflowStore,
    run_id: &str,
    workflow: &str,
    opts: &WaitOptions<'_>,
) -> Result<(), WorkflowError> {
    let direct = SignalAddress::Run {
        run_id: run_id.to_owned(),
        wait: opts.name.to_owned(),
    };
    let mut addresses = vec![direct.clone()];
    if let Some(ch) = opts.channel {
        addresses.push(SignalAddress::Channel {
            workflow: workflow.to_owned(),
            channel: ch.to_owned(),
        });
    }
    let mut passed = HashSet::new();
    loop {
        let Some(early) = store.mail_next(&addresses, &passed)? else {
            return Ok(());
        };
        let candidate: Result<Value, _> = serde_json::from_str(&early.value_json);
        let fits = match candidate {
            Ok(ref val) => check_wait_value(opts, val).is_ok(),
            Err(_) => false,
        };
        if !fits {
            if matches!(early.address, SignalAddress::Run { .. }) {
                store.mail_delete(&early.signal_id)?;
            }
            passed.insert(early.signal_id);
            continue;
        }
        let val = serde_json::from_str::<Value>(&early.value_json).unwrap_or(Value::Null);
        let woke = serde_json::json!({ "expired": false, "value": val });
        match store.mail_deliver(&early.signal_id, run_id, opts.name, &woke)? {
            MailDeliverOutcome::Gone => {}
            MailDeliverOutcome::Claimed | MailDeliverOutcome::Delivered => return Ok(()),
        }
    }
}

fn check_wait_value(opts: &WaitOptions<'_>, value: &Value) -> Result<Value, String> {
    validate_json_schema_value(opts.schema, value, true)?;
    match opts.codec {
        Some(codec) => codec(value),
        None => Ok(value.clone()),
    }
}

/// Builds the JSON Schema and request payload for `ask(name, title, choices, data)`.
pub fn ask_schema_and_request(
    title: &str,
    choices: &[String],
    data: Option<&Value>,
) -> (Map<String, Value>, Value) {
    let choice_vals: Vec<Value> = choices.iter().cloned().map(Value::String).collect();
    let mut props = Map::new();
    props.insert(
        "choice".to_owned(),
        serde_json::json!({ "type": "string", "enum": choice_vals }),
    );
    props.insert("reason".to_owned(), serde_json::json!({ "type": "string" }));
    let mut schema = Map::new();
    schema.insert("type".to_owned(), Value::String("object".to_owned()));
    schema.insert("properties".to_owned(), Value::Object(props));
    schema.insert("required".to_owned(), serde_json::json!(["choice"]));
    schema.insert("additionalProperties".to_owned(), Value::Bool(false));

    let mut req = Map::new();
    req.insert("title".to_owned(), Value::String(title.to_owned()));
    req.insert("choices".to_owned(), serde_json::json!(choices));
    if let Some(d) = data {
        req.insert("data".to_owned(), d.clone());
    }
    (schema, Value::Object(req))
}

/// Builds the [`PullRequestWatch`] and JSON Schema for `pullRequests.wait`.
pub fn pr_watch_schema(
    until: PullRequestUntil,
    pull_request: PullRequestAddress,
) -> (Map<String, Value>, PullRequestWatch) {
    let mut props = Map::new();
    props.insert(
        "state".to_owned(),
        serde_json::json!({ "type": "string", "enum": ["open", "closed", "merged"] }),
    );
    props.insert(
        "checks".to_owned(),
        serde_json::json!({ "type": "string", "enum": ["passing", "failing", "pending", "none"] }),
    );
    props.insert(
        "mergeability".to_owned(),
        serde_json::json!({ "type": "string", "enum": ["mergeable", "conflicting", "unknown"] }),
    );
    props.insert("url".to_owned(), serde_json::json!({ "type": "string" }));
    let mut schema = Map::new();
    schema.insert("type".to_owned(), Value::String("object".to_owned()));
    schema.insert("properties".to_owned(), Value::Object(props));
    schema.insert(
        "required".to_owned(),
        serde_json::json!(["state", "checks", "mergeability", "url"]),
    );
    let normalized_addr = match pull_request {
        PullRequestAddress::Url { project, url } => PullRequestAddress::Url { project, url },
        PullRequestAddress::Ref {
            project_id,
            host,
            repository,
            number,
        } => PullRequestAddress::Ref {
            project_id,
            host,
            repository,
            number,
        },
    };
    let watch = PullRequestWatch {
        kind: "pullRequest".to_owned(),
        until,
        pull_request: normalized_addr,
    };
    (schema, watch)
}

/// Helper to deserialize wait row `watch_json` if present.
pub fn parse_watch(row: &WaitRow) -> Option<PullRequestWatch> {
    row.watch_json
        .as_deref()
        .and_then(|raw| serde_json::from_str::<PullRequestWatch>(raw).ok())
}

fn storage_err(err: impl std::fmt::Display) -> WorkflowError {
    WorkflowError::Storage {
        message: err.to_string(),
    }
}
