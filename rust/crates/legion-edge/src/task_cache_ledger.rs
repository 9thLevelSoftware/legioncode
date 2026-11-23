//! Atomic D1 batch byte, row, hit/miss/eviction, and savings receipt accounting.

use std::collections::BTreeSet;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::task_cache_ledger_types::{
    D1BatchError, D1BatchReceipt, D1LedgerOp, DEFAULT_ASSOCIATION_LIMIT, DEFAULT_BYTE_LIMIT,
    DEFAULT_ENTRY_LIMIT,
};
use crate::task_cache_savings::{SavingsKey, SavingsTotals, SummaryRecord, TaskSavingsSource};

/// Atomic D1 ledger tracking cache hit/miss/eviction byte & row accounting and savings receipts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct D1BatchLedger {
    byte_limit: u64,
    entry_limit: u64,
    association_limit: u64,
    charged_bytes: u64,
    entry_count: u64,
    association_count: u64,
    hits: u64,
    fallback_hits: u64,
    misses: u64,
    evictions: u64,
    evicted_bytes: u64,
    rows_read: u64,
    rows_written: u64,
    batches_committed: u64,
    receipts: BTreeSet<String>,
    savings: IndexMap<SavingsKey, SavingsTotals>,
}

impl Default for D1BatchLedger {
    fn default() -> Self {
        Self::new(
            DEFAULT_BYTE_LIMIT,
            DEFAULT_ENTRY_LIMIT,
            DEFAULT_ASSOCIATION_LIMIT,
        )
    }
}

impl D1BatchLedger {
    /// Creates an empty ledger enforcing the given byte, entry, and association limits.
    pub fn new(byte_limit: u64, entry_limit: u64, association_limit: u64) -> Self {
        Self {
            byte_limit,
            entry_limit,
            association_limit,
            charged_bytes: 0,
            entry_count: 0,
            association_count: 0,
            hits: 0,
            fallback_hits: 0,
            misses: 0,
            evictions: 0,
            evicted_bytes: 0,
            rows_read: 0,
            rows_written: 0,
            batches_committed: 0,
            receipts: BTreeSet::new(),
            savings: IndexMap::new(),
        }
    }

    /// Currently charged bytes across active and uploading generations.
    pub fn charged_bytes(&self) -> u64 {
        self.charged_bytes
    }

    /// Active primary cache entry row count.
    pub fn entry_count(&self) -> u64 {
        self.entry_count
    }

    /// Active fallback association row count.
    pub fn association_count(&self) -> u64 {
        self.association_count
    }

    /// Cumulative exact cache hits.
    pub fn hits(&self) -> u64 {
        self.hits
    }

    /// Cumulative fallback association cache hits.
    pub fn fallback_hits(&self) -> u64 {
        self.fallback_hits
    }

    /// Cumulative cache misses.
    pub fn misses(&self) -> u64 {
        self.misses
    }

    /// Cumulative evicted entry count.
    pub fn evictions(&self) -> u64 {
        self.evictions
    }

    /// Cumulative bytes released by eviction/retirement.
    pub fn evicted_bytes(&self) -> u64 {
        self.evicted_bytes
    }

    /// Cumulative D1 rows read.
    pub fn rows_read(&self) -> u64 {
        self.rows_read
    }

    /// Cumulative D1 rows written.
    pub fn rows_written(&self) -> u64 {
        self.rows_written
    }

    /// Number of committed atomic batches.
    pub fn batches_committed(&self) -> u64 {
        self.batches_committed
    }

    /// Looks up accumulated lifetime savings totals for a `(developer, host, platform, arch, ci)` key.
    pub fn savings_for(&self, key: &SavingsKey) -> Option<&SavingsTotals> {
        self.savings.get(key)
    }

    /// Executes a slice of [`D1LedgerOp`] atomically: either every operation succeeds and commits
    /// together, or an error rolls back all staged mutations cleanly.
    pub fn execute_batch(&mut self, ops: &[D1LedgerOp]) -> Result<D1BatchReceipt, D1BatchError> {
        let mut staged = self.clone();
        let mut receipt = D1BatchReceipt::default();

        for op in ops {
            staged.apply_single(op, &mut receipt)?;
        }
        staged.batches_committed = add_checked(staged.batches_committed, 1, "batches_committed")?;

        *self = staged;
        Ok(receipt)
    }

    fn apply_single(
        &mut self,
        op: &D1LedgerOp,
        receipt: &mut D1BatchReceipt,
    ) -> Result<(), D1BatchError> {
        match op {
            D1LedgerOp::Reserve { bytes } => {
                let next_bytes = add_checked(self.charged_bytes, *bytes, "charged_bytes")?;
                if next_bytes > self.byte_limit {
                    return Err(D1BatchError::ByteLimitExceeded {
                        attempted: next_bytes,
                        limit: self.byte_limit,
                    });
                }
                self.charged_bytes = next_bytes;
                self.add_rows_written(1, receipt)?;
            }
            D1LedgerOp::Publish {
                key_is_new,
                association_is_new,
                actual_bytes,
                reserved_bytes,
                retired_bytes,
            } => self.apply_publish(
                *key_is_new,
                *association_is_new,
                *actual_bytes,
                *reserved_bytes,
                *retired_bytes,
                receipt,
            )?,
            D1LedgerOp::Evict {
                bytes,
                entries,
                associations,
            } => {
                self.charged_bytes = sub_checked(self.charged_bytes, *bytes, "charged_bytes")?;
                self.entry_count = sub_checked(self.entry_count, *entries, "entry_count")?;
                self.association_count =
                    sub_checked(self.association_count, *associations, "association_count")?;
                self.evictions = add_checked(self.evictions, *entries, "evictions")?;
                self.evicted_bytes = add_checked(self.evicted_bytes, *bytes, "evicted_bytes")?;
                let written = 1_u64.saturating_add(*entries).saturating_add(*associations);
                self.add_rows_written(written, receipt)?;
            }
            D1LedgerOp::RecordHit {
                fallback,
                rows_read,
            } => {
                if *fallback {
                    self.fallback_hits = add_checked(self.fallback_hits, 1, "fallback_hits")?;
                } else {
                    self.hits = add_checked(self.hits, 1, "hits")?;
                }
                self.add_rows_read(*rows_read, receipt)?;
            }
            D1LedgerOp::RecordMiss { rows_read } => {
                self.misses = add_checked(self.misses, 1, "misses")?;
                self.add_rows_read(*rows_read, receipt)?;
            }
            D1LedgerOp::RecordSavings { summary } => self.apply_savings(summary, receipt)?,
        }
        Ok(())
    }

    fn apply_publish(
        &mut self,
        key_is_new: bool,
        association_is_new: bool,
        actual_bytes: u64,
        reserved_bytes: u64,
        retired_bytes: Option<u64>,
        receipt: &mut D1BatchReceipt,
    ) -> Result<(), D1BatchError> {
        if key_is_new {
            let next_entries = add_checked(self.entry_count, 1, "entry_count")?;
            if next_entries > self.entry_limit {
                return Err(D1BatchError::EntryLimitExceeded {
                    attempted: next_entries,
                    limit: self.entry_limit,
                });
            }
            self.entry_count = next_entries;
        }
        if association_is_new {
            let next_assoc = add_checked(self.association_count, 1, "association_count")?;
            if next_assoc > self.association_limit {
                return Err(D1BatchError::AssociationLimitExceeded {
                    attempted: next_assoc,
                    limit: self.association_limit,
                });
            }
            self.association_count = next_assoc;
        }
        let after_reserve = sub_checked(self.charged_bytes, reserved_bytes, "charged_bytes")?;
        let with_actual = add_checked(after_reserve, actual_bytes, "charged_bytes")?;
        let after_retire = if let Some(retired) = retired_bytes {
            self.evictions = add_checked(self.evictions, 1, "evictions")?;
            self.evicted_bytes = add_checked(self.evicted_bytes, retired, "evicted_bytes")?;
            sub_checked(with_actual, retired, "charged_bytes")?
        } else {
            with_actual
        };
        if after_retire > self.byte_limit {
            return Err(D1BatchError::ByteLimitExceeded {
                attempted: after_retire,
                limit: self.byte_limit,
            });
        }
        self.charged_bytes = after_retire;
        let written = 1
            + u64::from(key_is_new)
            + u64::from(association_is_new)
            + u64::from(retired_bytes.is_some());
        self.add_rows_written(written, receipt)?;
        receipt.entries_applied += 1;
        Ok(())
    }

    fn apply_savings(
        &mut self,
        summary: &SummaryRecord,
        receipt: &mut D1BatchReceipt,
    ) -> Result<(), D1BatchError> {
        if !self.receipts.insert(summary.id.clone()) {
            return Ok(());
        }
        let key = SavingsKey {
            developer: summary.developer.clone(),
            host: summary.host.clone(),
            platform: summary.platform.clone(),
            arch: summary.arch.clone(),
            ci: summary.ci,
        };
        let totals = self.savings.entry(key).or_default();
        for task in &summary.tasks {
            match task.source {
                TaskSavingsSource::Local => {
                    totals.local_hits = add_checked(totals.local_hits, 1, "local_hits")?;
                    totals.local_ms = add_checked(totals.local_ms, task.saved_ms, "local_ms")?;
                }
                TaskSavingsSource::Remote => {
                    totals.remote_hits = add_checked(totals.remote_hits, 1, "remote_hits")?;
                    totals.remote_ms = add_checked(totals.remote_ms, task.saved_ms, "remote_ms")?;
                }
                TaskSavingsSource::Unspecified => {
                    totals.unspecified_hits =
                        add_checked(totals.unspecified_hits, 1, "unspecified_hits")?;
                    totals.unspecified_ms =
                        add_checked(totals.unspecified_ms, task.saved_ms, "unspecified_ms")?;
                }
                TaskSavingsSource::Executed => {
                    totals.executed = add_checked(totals.executed, 1, "executed")?;
                }
                TaskSavingsSource::Failed => {
                    totals.restore_failures =
                        add_checked(totals.restore_failures, 1, "restore_failures")?;
                }
            }
        }
        self.add_rows_written(2, receipt)?;
        receipt.savings_receipts_inserted += 1;
        Ok(())
    }

    fn add_rows_read(
        &mut self,
        delta: u64,
        receipt: &mut D1BatchReceipt,
    ) -> Result<(), D1BatchError> {
        self.rows_read = add_checked(self.rows_read, delta, "rows_read")?;
        receipt.rows_read_delta = add_checked(receipt.rows_read_delta, delta, "rows_read_delta")?;
        Ok(())
    }

    fn add_rows_written(
        &mut self,
        delta: u64,
        receipt: &mut D1BatchReceipt,
    ) -> Result<(), D1BatchError> {
        self.rows_written = add_checked(self.rows_written, delta, "rows_written")?;
        receipt.rows_written_delta =
            add_checked(receipt.rows_written_delta, delta, "rows_written_delta")?;
        Ok(())
    }
}

fn add_checked(current: u64, delta: u64, field: &'static str) -> Result<u64, D1BatchError> {
    current
        .checked_add(delta)
        .ok_or(D1BatchError::ArithmeticOverflow { field })
}

fn sub_checked(current: u64, delta: u64, field: &'static str) -> Result<u64, D1BatchError> {
    current.checked_sub(delta).ok_or(D1BatchError::Underflow {
        field,
        current,
        delta,
    })
}
