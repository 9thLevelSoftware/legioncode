//! Scheduled Cloudflare Worker cron planning (`"* * * * *"` and `"0 * * * *"`) matching
//! `infra/task-cache/src/worker.ts` and `Cleanup.ts`.

use serde::{Deserialize, Serialize};

use crate::cf_bindings_d1::{D1BindingPlan, MAX_GC_BATCH_SIZE, SAVINGS_RETENTION_MS};

/// Cron schedule classification recognized by `run_scheduled_cron`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CronScheduleKind {
    /// Every-minute `INDEX` expired cache generation garbage collection (`"* * * * *"`).
    EveryMinuteCacheGc,
    /// Hourly `SAVINGS` receipt pruning & lifetime totals aggregation (`"0 * * * *"`).
    HourlySavingsMaintenance,
    /// Unrecognized cron trigger expression.
    Unknown,
}

/// Deterministic cron execution plan emitted by `run_scheduled_cron`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledCronReport {
    /// Raw Cloudflare cron expression string.
    pub cron_expr: String,
    /// Classified cron schedule handler.
    pub schedule_kind: CronScheduleKind,
    /// Whether `cron_expr` matched a registered Worker cron trigger.
    pub recognized: bool,
    /// Explicit evaluation timestamp in epoch milliseconds.
    pub now_ms: u64,
    /// Effective D1 GC batch size clamped into `1..=256`.
    pub gc_batch_size: u32,
    /// Retention cutoff epoch milliseconds (`now_ms - 31 days` for hourly savings cleanup).
    pub retention_cutoff_ms: Option<u64>,
    /// Ordered parameterized D1 statements scheduled for execution.
    pub d1_plans: Vec<D1BindingPlan>,
    /// Human-readable operational summary for logs and spans.
    pub summary: String,
}

/// Plans D1 maintenance queries for `infra/task-cache` scheduled Worker cron triggers.
///
/// Supports `"* * * * *"` (three bounded expired-generation GC batches up to `gc_batch_size`
/// clamped to `1..=256`, matching `Cleanup.ts`) and `"0 * * * *"` (pruning expired `SAVINGS`
/// receipts older than `31 days` and summing lifetime totals).
pub fn run_scheduled_cron(cron_expr: &str, now_ms: u64, gc_batch_size: u32) -> ScheduledCronReport {
    let trimmed = cron_expr.trim();
    let clamped_batch = gc_batch_size.clamp(1, MAX_GC_BATCH_SIZE);
    let now_secs = now_ms / 1_000;

    match trimmed {
        "* * * * *" => {
            let mut d1_plans = Vec::with_capacity(6);
            for round in 1..=3_u32 {
                let claim_id = format!("gc-{now_secs}-{round}");
                d1_plans.push(D1BindingPlan::claim_expired_gc_batch(
                    &claim_id,
                    now_secs,
                    clamped_batch,
                ));
                d1_plans.push(D1BindingPlan::delete_expired_gc_batch(
                    now_secs,
                    clamped_batch,
                ));
            }
            ScheduledCronReport {
                cron_expr: trimmed.to_owned(),
                schedule_kind: CronScheduleKind::EveryMinuteCacheGc,
                recognized: true,
                now_ms,
                gc_batch_size: clamped_batch,
                retention_cutoff_ms: None,
                d1_plans,
                summary: format!(
                    "planned 3 expired generation GC claim+delete rounds (batch={clamped_batch})"
                ),
            }
        }
        "0 * * * *" => {
            let cutoff_ms = now_ms.saturating_sub(SAVINGS_RETENTION_MS);
            let d1_plans = vec![
                D1BindingPlan::delete_expired_savings_receipts(now_ms),
                D1BindingPlan::select_savings_sum(None),
            ];
            ScheduledCronReport {
                cron_expr: trimmed.to_owned(),
                schedule_kind: CronScheduleKind::HourlySavingsMaintenance,
                recognized: true,
                now_ms,
                gc_batch_size: clamped_batch,
                retention_cutoff_ms: Some(cutoff_ms),
                d1_plans,
                summary: format!(
                    "planned hourly SAVINGS receipt retention prune (< {cutoff_ms}ms) and totals sum"
                ),
            }
        }
        _ => ScheduledCronReport {
            cron_expr: trimmed.to_owned(),
            schedule_kind: CronScheduleKind::Unknown,
            recognized: false,
            now_ms,
            gc_batch_size: clamped_batch,
            retention_cutoff_ms: None,
            d1_plans: Vec::new(),
            summary: format!("unrecognized cron expression '{trimmed}'"),
        },
    }
}
