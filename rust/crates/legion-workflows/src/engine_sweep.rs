#![forbid(unsafe_code)]
//! Periodic sweep execution (wait timeouts, sleep wakeups, PR watches, triggers, pruning) and webhook delivery.

use std::sync::Arc;

use serde_json::Value;
use uuid::Uuid;

use crate::engine::{WorkflowDefinition, WorkflowEngine, set_contains};
use crate::engine_replay::ReplayContext;
use crate::engine_step::{
    END_BACKOFF_MS, END_MAX_TRIES, END_STEP, PassControl, RunEndInfo, StepRetry,
};
use crate::journal::{JOURNAL_PRUNE_AFTER_MS, JOURNAL_PRUNE_BATCH};
use crate::time::{format_millis_iso, now_epoch_ms, parse_iso_millis};
use crate::triggers_hooks::{HookDeliveryInput, constant_time_equals, resolve_webhook_key};
use crate::types::{WorkflowError, WorkflowRunStatus};
use crate::waits_mail::WAKE_REDONE_AFTER_MS;

impl WorkflowEngine {
    /// Handles an inbound webhook request routed to `delivery.hook_id`.
    pub async fn deliver(
        self: &Arc<Self>,
        delivery: HookDeliveryInput,
    ) -> Result<Option<String>, WorkflowError> {
        if !self.is_enabled() || self.definitions.is_empty() {
            return Ok(None);
        }
        let Some(hook) = self.hooks.iter().find(|h| h.hook_id == delivery.hook_id) else {
            return Ok(None);
        };
        if !constant_time_equals(&delivery.token, &hook.token) {
            return Ok(None);
        }
        let authored = match &hook.key_fn {
            Some(f) => f(&delivery.request).map_err(|problem| WorkflowError::Input {
                problem: format!(
                    "The key of hook {} could not be read from the request: {problem}",
                    hook.name
                ),
            })?,
            None => None,
        };
        let fallback_uuid = Uuid::new_v4().to_string();
        let key = resolve_webhook_key(
            authored,
            &delivery.request,
            delivery.delivery_id.as_deref(),
            &fallback_uuid,
        );
        let idemp_key = format!("hook:{}:{key}", hook.hook_id);
        let payload =
            serde_json::to_value(&delivery.request).map_err(|e| WorkflowError::Storage {
                message: e.to_string(),
            })?;
        let run_id = self
            .start(&hook.workflow, payload, &idemp_key, None, None)
            .await?;
        Ok(Some(run_id))
    }

    /// Runs one periodic sweep: wait timeouts, sleep wakeups, PR watches, schedules, opened items,
    /// orphaned/cut-short cancels, automatic resumes, mail expiry, and 1h journal pruning.
    pub async fn sweep(self: &Arc<Self>) -> Result<(), WorkflowError> {
        if !self.is_enabled() {
            return Ok(());
        }
        let now_ms = now_epoch_ms();
        let now_iso = format_millis_iso(now_ms);
        for wait in self.store.waits_past_deadline(&now_iso)? {
            let woke = serde_json::json!({ "expired": true, "value": Value::Null });
            if self
                .store
                .wait_signalled(&wait.run_id, &wait.name, &woke, None)?
                && !self.is_paused_or_resuming(&wait.run_id)?
            {
                let _ = self.drive_run(&wait.run_id).await;
            }
        }
        let stale_before = format_millis_iso(now_ms.saturating_sub(WAKE_REDONE_AFTER_MS));
        for wait in self.store.waits_to_wake(&stale_before)? {
            if !self.is_paused_or_resuming(&wait.run_id)? {
                let _ = self.drive_run(&wait.run_id).await;
            }
        }
        for (run_id, _sleep_key) in self.journal.due_sleeps(&now_iso)? {
            if !self.is_paused_or_resuming(&run_id)? {
                let _ = self.drive_run(&run_id).await;
            }
        }
        for orphan in self.store.orphans()? {
            let _ = self
                .cancel(
                    &orphan.run_id,
                    Some("the run that invoked it was cancelled".to_owned()),
                )
                .await;
        }
        for row in self.store.cancels_cut_short(END_STEP)? {
            let busy = set_contains(&self.cancelling, &row.run_id)
                || set_contains(&self.ending, &row.run_id);
            if busy {
                continue;
            }
            let threads = self.store.running_threads(&row.run_id)?;
            self.stop_steps(&row.run_id, &threads).await;
        }
        for child in self.store.untold()? {
            self.notify_parent_ended(&child.run_id).await;
        }
        self.store.mail_expire(&now_iso)?;
        self.fire_schedules(now_ms).await;
        self.fire_opened(now_ms).await;
        self.fire_watches(now_ms).await;
        for row in self.store.runs_to_end(END_STEP, END_MAX_TRIES)? {
            let backoff_idx = usize::try_from(row.end_attempts.saturating_sub(1)).ok();
            if let Some(ended_at) = row.end_ended_at.as_deref()
                && let Some(delay) = backoff_idx.and_then(|i| END_BACKOFF_MS.get(i))
                && let Some(ended_ms) = parse_iso_millis(ended_at)
                && now_ms < ended_ms.saturating_add(*delay)
            {
                continue;
            }
            if !self.in_flight.busy(&row.run.run_id) {
                self.end_cancelled(&row.run).await;
            }
        }
        for paused in self.store.every_run(WorkflowRunStatus::Paused)? {
            if let Some(due_ms) = self.resumes_at_ms(&paused)
                && now_ms >= due_ms
            {
                let _ = self
                    .resume_internal(
                        &paused.run_id,
                        Some((paused.pauses, paused.updated_at.clone())),
                    )
                    .await;
            }
        }
        let _ = self
            .journal
            .prune_ended_runs(now_ms, JOURNAL_PRUNE_AFTER_MS, JOURNAL_PRUNE_BATCH);
        Ok(())
    }

    pub(crate) async fn run_on_end_in_body(
        self: &Arc<Self>,
        ctx: &mut ReplayContext,
        def: &WorkflowDefinition,
        input: Value,
        outcome: &str,
        reason: Option<&str>,
    ) -> Result<bool, WorkflowError> {
        if !def.has_on_end {
            return Ok(true);
        }
        if ctx.claim(END_STEP, true).is_err() {
            return Ok(false);
        }
        let end_info = RunEndInfo {
            id: ctx.run_id.clone(),
            outcome: outcome.to_owned(),
            reason: reason.map(str::to_owned),
        };
        let retry = StepRetry {
            times: 3,
            delay_ms: 50,
        };
        let driver = Arc::clone(&def.driver);
        let res = ctx
            .run_step_internal(END_STEP, None, retry, false, &mut |sctx| {
                let d = Arc::clone(&driver);
                let p = input.clone();
                let info = end_info.clone();
                Box::pin(async move { d.run_on_end(sctx, p, info).await.map(|()| Value::Null) })
            })
            .await;
        match res {
            Ok(_) => Ok(true),
            Err(PassControl::Suspend) => Ok(false),
            Err(PassControl::Fault(msg) | PassControl::Stop { reason: msg, .. }) => {
                self.store.run_paused(&ctx.run_id, &msg, None)?;
                Ok(false)
            }
        }
    }
}
