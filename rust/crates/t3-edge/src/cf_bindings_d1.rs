//! Typed Cloudflare D1 SQL query plans (`INDEX`, `SAVINGS`) for `infra/task-cache`.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::task_cache::{SummaryRecord, TaskSavingsSource};

/// Maximum number of generation rows claimed or deleted in one cron GC batch (`Cleanup.ts:4`).
pub const MAX_GC_BATCH_SIZE: u32 = 256;

/// Lease extension seconds while deleting claimed cache generations (`database.ts:230`).
pub const GC_CLAIM_LEASE_SECONDS: u64 = 600;

/// Retention duration in milliseconds for `SAVINGS` deduplication receipts (`31 days`).
pub const SAVINGS_RETENTION_MS: u64 = 31 * 86_400_000;

/// Target Cloudflare D1 binding identifier for `infra/task-cache`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum D1DatabaseBinding {
    /// Primary cache policy, generation, and key index database (`INDEX`).
    Index,
    /// Lifetime task savings and receipt deduplication database (`SAVINGS`).
    Savings,
}

impl D1DatabaseBinding {
    /// Canonical Wrangler/Alchemy binding name (`"INDEX"` or `"SAVINGS"`).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Index => "INDEX",
            Self::Savings => "SAVINGS",
        }
    }
}

impl fmt::Display for D1DatabaseBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Parameterized Cloudflare D1 query ready for host binding execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct D1BindingPlan {
    /// Target D1 database binding (`"INDEX"` or `"SAVINGS"`).
    pub database: D1DatabaseBinding,
    /// Parameterized SQLite statement using `?` placeholders.
    pub sql: String,
    /// Ordered positional SQL bind values.
    pub params: Vec<Value>,
}

impl D1BindingPlan {
    /// Creates an `INDEX` parameterized `SELECT` looking up an exact or fallback cache entry.
    pub fn select_artifact_entry(
        scope_id: &str,
        primary_key_hex: &str,
        secondary_key_hex: &str,
        now_secs: u64,
    ) -> Self {
        Self {
            database: D1DatabaseBinding::Index,
            sql: "SELECT g.generation_id, g.value_object, g.blob_object, g.blob_id, \
                  g.value_size, g.blob_size, e.key, 'exact' AS kind, 0 AS priority \
                  FROM entries e \
                  JOIN generations g ON g.generation_id = e.generation_id \
                  JOIN scopes s ON s.scope_id = e.scope_id CROSS JOIN deployment d \
                  WHERE e.scope_id = ?1 AND e.key = unhex(?2) AND g.state = 'ready' \
                  AND g.expires_at > ?4 AND s.enabled = 1 AND d.enabled = 1 \
                  UNION ALL \
                  SELECT g.generation_id, g.value_object, g.blob_object, g.blob_id, \
                  g.value_size, g.blob_size, e.key, 'fallback' AS kind, 1 AS priority \
                  FROM associations a \
                  JOIN entries e ON e.scope_id = a.scope_id AND e.key = a.target_key \
                  JOIN generations g ON g.generation_id = e.generation_id \
                  JOIN scopes s ON s.scope_id = e.scope_id CROSS JOIN deployment d \
                  WHERE a.scope_id = ?1 AND a.secondary_key = unhex(?3) AND g.state = 'ready' \
                  AND g.expires_at > ?4 AND s.enabled = 1 AND d.enabled = 1 \
                  ORDER BY priority LIMIT 1"
                .to_owned(),
            params: vec![
                json!(scope_id),
                json!(primary_key_hex),
                json!(secondary_key_hex),
                json!(now_secs),
            ],
        }
    }

    /// Creates an `INDEX` `INSERT OR REPLACE` query binding an artifact entry key to `generation_id`.
    pub fn upsert_artifact_entry(scope_id: &str, key_hex: &str, generation_id: &str) -> Self {
        Self {
            database: D1DatabaseBinding::Index,
            sql: "INSERT OR REPLACE INTO entries (scope_id, key, generation_id) \
                  VALUES (?1, unhex(?2), ?3)"
                .to_owned(),
            params: vec![json!(scope_id), json!(key_hex), json!(generation_id)],
        }
    }

    /// Creates an `INDEX` expired-generation claim update before R2 object deletion (`Cleanup.ts`).
    pub fn claim_expired_gc_batch(claim_id: &str, now_secs: u64, batch_size: u32) -> Self {
        let clamped = batch_size.clamp(1, MAX_GC_BATCH_SIZE);
        let claim_until = now_secs.saturating_add(GC_CLAIM_LEASE_SECONDS);
        Self {
            database: D1DatabaseBinding::Index,
            sql: "UPDATE generations SET state = 'deleting', gc_claim = ?1, gc_after = ?2 \
                  WHERE generation_id IN (\
                  SELECT generation_id FROM generations WHERE gc_after <= ?3 \
                  ORDER BY gc_after LIMIT ?4) \
                  RETURNING generation_id, value_object, blob_object, upload_id"
                .to_owned(),
            params: vec![
                json!(claim_id),
                json!(claim_until),
                json!(now_secs),
                json!(clamped),
            ],
        }
    }

    /// Creates an `INDEX` batched `DELETE` removing expired cache generations (`batch_size` `1..=256`).
    pub fn delete_expired_gc_batch(now_secs: u64, batch_size: u32) -> Self {
        let clamped = batch_size.clamp(1, MAX_GC_BATCH_SIZE);
        Self {
            database: D1DatabaseBinding::Index,
            sql: "DELETE FROM generations WHERE generation_id IN (\
                  SELECT generation_id FROM generations WHERE gc_after <= ?1 \
                  ORDER BY gc_after LIMIT ?2)"
                .to_owned(),
            params: vec![json!(now_secs), json!(clamped)],
        }
    }

    /// Creates a `SAVINGS` receipt insertion (`INSERT INTO receipts ... ON CONFLICT DO NOTHING`).
    pub fn insert_savings_receipt(summary_id: &str, recorded_at_ms: u64) -> Self {
        Self {
            database: D1DatabaseBinding::Savings,
            sql: "INSERT INTO receipts (id, recorded_at) VALUES (?1, ?2) ON CONFLICT DO NOTHING"
                .to_owned(),
            params: vec![json!(summary_id), json!(recorded_at_ms)],
        }
    }

    /// Creates a `SAVINGS` upsert statement aggregating one [`SummaryRecord`] into lifetime counters.
    pub fn upsert_savings_rollup(summary: &SummaryRecord, developer: &str) -> Self {
        let mut local_ms = 0_u64;
        let mut remote_ms = 0_u64;
        let mut unspecified_ms = 0_u64;
        let mut local_hits = 0_u64;
        let mut remote_hits = 0_u64;
        let mut unspecified_hits = 0_u64;
        let mut executed = 0_u64;
        let mut restore_failures = 0_u64;

        for entry in &summary.tasks {
            match entry.source {
                TaskSavingsSource::Local => {
                    local_ms = local_ms.saturating_add(entry.saved_ms);
                    local_hits = local_hits.saturating_add(1);
                }
                TaskSavingsSource::Remote => {
                    remote_ms = remote_ms.saturating_add(entry.saved_ms);
                    remote_hits = remote_hits.saturating_add(1);
                }
                TaskSavingsSource::Unspecified => {
                    unspecified_ms = unspecified_ms.saturating_add(entry.saved_ms);
                    unspecified_hits = unspecified_hits.saturating_add(1);
                }
                TaskSavingsSource::Executed => executed = executed.saturating_add(1),
                TaskSavingsSource::Failed => restore_failures = restore_failures.saturating_add(1),
            }
        }

        Self {
            database: D1DatabaseBinding::Savings,
            sql: "INSERT INTO savings (developer, host, platform, arch, ci, local_ms, remote_ms, \
                  local_hits, remote_hits, executed, restore_failures, unspecified_ms, \
                  unspecified_hits) \
                  VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13) \
                  ON CONFLICT (developer, host, platform, arch, ci) DO UPDATE SET \
                  local_ms = local_ms + excluded.local_ms, \
                  remote_ms = remote_ms + excluded.remote_ms, \
                  local_hits = local_hits + excluded.local_hits, \
                  remote_hits = remote_hits + excluded.remote_hits, \
                  executed = executed + excluded.executed, \
                  restore_failures = restore_failures + excluded.restore_failures, \
                  unspecified_ms = unspecified_ms + excluded.unspecified_ms, \
                  unspecified_hits = unspecified_hits + excluded.unspecified_hits"
                .to_owned(),
            params: vec![
                json!(developer),
                json!(summary.host),
                json!(summary.platform),
                json!(summary.arch),
                json!(i64::from(summary.ci)),
                json!(local_ms),
                json!(remote_ms),
                json!(local_hits),
                json!(remote_hits),
                json!(executed),
                json!(restore_failures),
                json!(unspecified_ms),
                json!(unspecified_hits),
            ],
        }
    }

    /// Creates a `SAVINGS` retention cleanup `DELETE` pruning receipts older than `31 days`.
    pub fn delete_expired_savings_receipts(now_ms: u64) -> Self {
        let cutoff_ms = now_ms.saturating_sub(SAVINGS_RETENTION_MS);
        Self {
            database: D1DatabaseBinding::Savings,
            sql: "DELETE FROM receipts WHERE recorded_at < ?1".to_owned(),
            params: vec![json!(cutoff_ms)],
        }
    }

    /// Creates a `SAVINGS` aggregate `SELECT SUM(...)` rollup query, optionally scoped to `developer`.
    pub fn select_savings_sum(developer: Option<&str>) -> Self {
        let base = "SELECT COALESCE(SUM(local_ms), 0) AS local_ms, \
                    COALESCE(SUM(remote_ms), 0) AS remote_ms, \
                    COALESCE(SUM(unspecified_ms), 0) AS unspecified_ms, \
                    COALESCE(SUM(local_hits), 0) AS local_hits, \
                    COALESCE(SUM(remote_hits), 0) AS remote_hits, \
                    COALESCE(SUM(unspecified_hits), 0) AS unspecified_hits, \
                    COALESCE(SUM(executed), 0) AS executed, \
                    COALESCE(SUM(restore_failures), 0) AS restore_failures \
                    FROM savings";
        match developer {
            Some(dev) => Self {
                database: D1DatabaseBinding::Savings,
                sql: format!("{base} WHERE developer = ?1"),
                params: vec![json!(dev)],
            },
            None => Self {
                database: D1DatabaseBinding::Savings,
                sql: base.to_owned(),
                params: Vec::new(),
            },
        }
    }
}
