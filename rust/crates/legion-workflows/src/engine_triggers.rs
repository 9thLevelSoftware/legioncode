//! Periodic trigger polling, PR watch checks, and cancelled-run `onEnd` execution.

use std::sync::Arc;

use serde_json::Value;

use crate::engine::WorkflowEngine;
use crate::engine_queries::compute_latest_defs;
use crate::engine_step::{END_STEP, RunEndInfo, StepExecContext};
use crate::engine_wait::parse_watch;
use crate::store::RunRow;
use crate::time::{
    format_millis_iso, is_missed_fixed_time_run, next_scheduled_run_ms, parse_iso_millis,
};
use crate::traits::{
    AgentInterruptTarget, IssueQuery, PullRequestQuery, is_pr_settled, standing_of,
};
use crate::triggers_hooks::{TriggerDef, reference_from_opened_entry};
use crate::types::{SignalAddress, WorkflowError, WorkflowStepKind, WorkflowStepStatus};

const LOOK_EVERY_MS: i64 = 60_000;
const LOOK_AT_MOST: u32 = 100;

impl WorkflowEngine {
    pub(crate) async fn fire_schedules(self: &Arc<Self>, now_ms: i64) {
        for def in compute_latest_defs(&self.definitions) {
            for trig in &def.triggers {
                let TriggerDef::Schedule { id, schedule } = trig else {
                    continue;
                };
                let full_id = format!("{}/{id}", def.name);
                if let Ok(Some(due)) = self.store.trigger_next(&full_id)
                    && let Some(due_ms) = parse_iso_millis(&due)
                {
                    if now_ms < due_ms {
                        continue;
                    }
                    if !is_missed_fixed_time_run(schedule, due_ms, now_ms) {
                        let key = format!("trigger:{full_id}:{due}");
                        let payload = serde_json::json!({ "scheduledAt": due });
                        if self
                            .start(&def.name, payload, &key, None, None)
                            .await
                            .is_err()
                        {
                            continue;
                        }
                    }
                }
                if let Some(next_ms) = next_scheduled_run_ms(schedule, now_ms) {
                    let _ = self
                        .store
                        .trigger_set(&full_id, &format_millis_iso(next_ms));
                }
            }
        }
    }

    pub(crate) async fn fire_opened(self: &Arc<Self>, now_ms: i64) {
        for def in compute_latest_defs(&self.definitions) {
            for trig in &def.triggers {
                let (id, is_issue, project, labels, query) = match trig {
                    TriggerDef::IssuesOpened {
                        id,
                        project,
                        labels,
                        query,
                    } => (id, true, project, Some(labels.clone()), query.clone()),
                    TriggerDef::PullRequestsOpened { id, project, query } => {
                        (id, false, project, None, query.clone())
                    }
                    _ => continue,
                };
                let full_id = format!("{}/{id}", def.name);
                let Ok(since_opt) = self.store.trigger_next(&full_id) else {
                    continue;
                };
                let Some(since) = since_opt else {
                    let _ = self.store.trigger_set(&full_id, &format_millis_iso(now_ms));
                    continue;
                };
                if !self.should_poll_now(&full_id, now_ms) {
                    continue;
                }
                let entries = if is_issue {
                    self.forge
                        .issues_list(IssueQuery {
                            project: project.clone(),
                            state: Some("all".to_owned()),
                            labels,
                            query,
                            assignee: None,
                            sort: Some("created".to_owned()),
                            limit: Some(LOOK_AT_MOST),
                        })
                        .await
                } else {
                    self.forge
                        .pull_requests_list(PullRequestQuery {
                            project: project.clone(),
                            state: Some("all".to_owned()),
                            query,
                            limit: Some(LOOK_AT_MOST),
                        })
                        .await
                };
                let Ok(items) = entries else {
                    continue;
                };
                let since_ms = parse_iso_millis(&since).unwrap_or(now_ms);
                let mut fresh: Vec<(i64, Value)> = items
                    .into_iter()
                    .filter_map(|item| {
                        let created = item
                            .get("createdAt")
                            .and_then(Value::as_str)
                            .and_then(parse_iso_millis)?;
                        (created >= since_ms).then_some((created, item))
                    })
                    .collect();
                fresh.sort_by_key(|(c, _)| *c);
                for (created_ms, entry) in fresh {
                    let url = entry.get("url").and_then(Value::as_str).unwrap_or("");
                    let key = format!("trigger:{full_id}:{url}");
                    let skipped = self.turned_down.lock().is_ok_and(|g| g.contains(&key));
                    if !skipped
                        && self
                            .store
                            .run_by_key(&def.name, &key)
                            .ok()
                            .flatten()
                            .is_none()
                    {
                        match self.start(&def.name, entry.clone(), &key, None, None).await {
                            Ok(run_id) => {
                                if let Some(ref_input) = reference_from_opened_entry(&entry) {
                                    let _ = self.store.reference_noted(&run_id, None, &ref_input);
                                }
                            }
                            Err(WorkflowError::Input { .. }) => {
                                if let Ok(mut g) = self.turned_down.lock() {
                                    g.insert(key);
                                }
                            }
                            Err(_) => {}
                        }
                    }
                    let _ = self
                        .store
                        .trigger_set(&full_id, &format_millis_iso(created_ms));
                }
            }
        }
    }

    pub(crate) async fn fire_watches(self: &Arc<Self>, now_ms: i64) {
        let Ok(watches) = self.store.watches() else {
            return;
        };
        for wait in watches {
            let id = format!("watch:{}/{}", wait.run_id, wait.name);
            if !self.should_poll_now(&id, now_ms) {
                continue;
            }
            let Some(watch) = parse_watch(&wait) else {
                continue;
            };
            let Ok(detail) = self.forge.pull_requests_read(watch.pull_request).await else {
                continue;
            };
            let standing = standing_of(&detail);
            let without = if let Ok(mut g) = self.no_checks_since.lock() {
                if standing.checks == "none" {
                    let first = *g.entry(id.clone()).or_insert(now_ms);
                    now_ms.saturating_sub(first)
                } else {
                    g.remove(&id);
                    0
                }
            } else {
                0
            };
            if !is_pr_settled(watch.until, &standing, without) {
                continue;
            }
            let standing_val = serde_json::to_value(&standing).unwrap_or(Value::Null);
            let woke = serde_json::json!({ "expired": false, "value": standing_val });
            if self
                .store
                .wait_signalled(&wait.run_id, &wait.name, &woke, None)
                .unwrap_or(false)
            {
                if let Ok(mut g) = self.looked_at.lock() {
                    g.remove(&id);
                }
                if let Ok(mut g) = self.no_checks_since.lock() {
                    g.remove(&id);
                }
                if !self.is_paused_or_resuming(&wait.run_id).unwrap_or(true) {
                    let _ = self.drive_run(&wait.run_id).await;
                }
            }
        }
    }

    pub(crate) async fn end_cancelled(self: &Arc<Self>, row: &RunRow) {
        let Some(def) = self.by_tag.get(&row.engine_tag).cloned() else {
            return;
        };
        if !def.has_on_end {
            return;
        }
        if let Ok(mut g) = self.ending.lock()
            && !g.insert(row.run_id.clone())
        {
            return;
        }
        self.in_flight.settled(&row.run_id).await;
        let already_done = self.store.steps(&row.run_id).is_ok_and(|steps| {
            steps
                .iter()
                .any(|s| s.name == END_STEP && s.status == WorkflowStepStatus::Completed)
        });
        if !already_done {
            let _ = self
                .store
                .step_started(&row.run_id, END_STEP, WorkflowStepKind::Step);
            let input: Value = row
                .input_json
                .as_deref()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or(Value::Null);
            let mut sctx = StepExecContext::new(
                self.store.clone(),
                Arc::clone(&self.forge),
                self.control_handle(),
                row.run_id.clone(),
                END_STEP,
            );
            let end_info = RunEndInfo {
                id: row.run_id.clone(),
                outcome: "cancelled".to_owned(),
                reason: row.detail.clone(),
            };
            match def.driver.run_on_end(&mut sctx, input, end_info).await {
                Ok(()) => {
                    let _ = self.store.step_ended(
                        &row.run_id,
                        END_STEP,
                        WorkflowStepStatus::Completed,
                        None,
                    );
                }
                Err(err) => {
                    let _ = self.store.step_ended(
                        &row.run_id,
                        END_STEP,
                        WorkflowStepStatus::Failed,
                        Some(&err),
                    );
                }
            }
        }
        if let Ok(mut g) = self.ending.lock() {
            g.remove(&row.run_id);
        }
    }

    pub(crate) async fn stop_steps(
        &self,
        run_id: &str,
        threads: &[crate::store_ops::RunningStepThread],
    ) {
        for thread in threads {
            let _ = self
                .agents
                .interrupt(AgentInterruptTarget {
                    thread_id: thread.thread_id.clone(),
                    environment: thread.environment.clone(),
                    turn_id: thread.turn_id.clone(),
                })
                .await;
        }
        let _ = self.store.steps_cancelled(run_id, END_STEP);
    }

    pub(crate) async fn notify_parent_ended(self: &Arc<Self>, run_id: &str) {
        let Ok(Some(row)) = self.store.run(run_id) else {
            return;
        };
        let (Some(parent_run_id), Some(parent_wait)) = (row.parent_run_id, row.parent_wait) else {
            return;
        };
        if row.ended_at.is_none() {
            return;
        }
        let result_val = row
            .result_json
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or(Value::Null);
        let mut msg = serde_json::json!({
            "outcome": row.status.as_str(),
            "value": result_val,
        });
        if let Some(detail) = row.detail
            && let Some(obj) = msg.as_object_mut()
        {
            obj.insert("reason".to_owned(), Value::String(detail));
        }
        let sid = format!("{run_id}/ended");
        let _ = self
            .signal(
                SignalAddress::Run {
                    run_id: parent_run_id,
                    wait: parent_wait,
                },
                msg,
                &sid,
                None,
            )
            .await;
    }

    fn should_poll_now(&self, id: &str, now_ms: i64) -> bool {
        let Ok(mut g) = self.looked_at.lock() else {
            return false;
        };
        if let Some(last) = g.get(id)
            && now_ms.saturating_sub(*last) < LOOK_EVERY_MS
        {
            return false;
        }
        g.insert(id.to_owned(), now_ms);
        true
    }
}
