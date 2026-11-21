//! Operations, receipts, and rollback error types for [`crate::task_cache::D1BatchLedger`].

use thiserror::Error;

use crate::task_cache_savings::SummaryRecord;

/// Default per-scope and deployment byte quota (`8 GB`, `0001_cache.sql:6`).
pub const DEFAULT_BYTE_LIMIT: u64 = 8_000_000_000;

/// Default entry count limit (`20_000`, `0001_cache.sql:7`).
pub const DEFAULT_ENTRY_LIMIT: u64 = 20_000;

/// Default association count limit (`20_000`, `0001_cache.sql:8`).
pub const DEFAULT_ASSOCIATION_LIMIT: u64 = 20_000;

/// Single operation inside an atomic `D1BatchLedger::execute_batch` transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum D1LedgerOp {
    /// Reserves capacity for an uploading generation (`reserve_capacity` + `charge_generation`).
    Reserve {
        /// Estimated bytes charged upfront.
        bytes: u64,
    },
    /// Publishes an artifact generation, optionally retiring an overwritten entry and/or creating
    /// a new entry or association row.
    Publish {
        /// `true` when inserting a new primary key row in `entries`.
        key_is_new: bool,
        /// `true` when inserting a new fallback row in `associations`.
        association_is_new: bool,
        /// Actual published bytes (`value_size + blob_size`).
        actual_bytes: u64,
        /// Previously reserved bytes replaced by `actual_bytes`.
        reserved_bytes: u64,
        /// Previous ready generation bytes immediately retired/evicted, if any.
        retired_bytes: Option<u64>,
    },
    /// Evicts / deletes entries, associations, and charged bytes (`release_generation` / GC).
    Evict {
        /// Bytes released from `charged_bytes`.
        bytes: u64,
        /// Primary entry rows removed.
        entries: u64,
        /// Fallback association rows removed.
        associations: u64,
    },
    /// Records an exact or fallback cache hit (`selectEntry`).
    RecordHit {
        /// `true` when matched via `associations` fallback.
        fallback: bool,
        /// Number of D1 rows scanned.
        rows_read: u64,
    },
    /// Records a cache miss (`selectEntry` returning null).
    RecordMiss {
        /// Number of D1 rows scanned.
        rows_read: u64,
    },
    /// Atomically deduplicates a receipt ID and updates lifetime `savings` (`Savings.ts:62-88`).
    RecordSavings {
        /// Telemetry summary payload.
        summary: SummaryRecord,
    },
}

/// Summary of mutations committed by one successful `D1BatchLedger::execute_batch` call.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct D1BatchReceipt {
    /// Total D1 rows read during the batch.
    pub rows_read_delta: u64,
    /// Total D1 rows written during the batch.
    pub rows_written_delta: u64,
    /// Net change in active entries published during the batch.
    pub entries_applied: usize,
    /// Number of newly accepted (non-duplicate) savings receipts.
    pub savings_receipts_inserted: usize,
}

/// Errors causing an entire `D1BatchLedger` batch to roll back with zero partial state leaks.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum D1BatchError {
    /// Charged bytes would exceed the configured byte quota.
    #[error("charged bytes {attempted} exceed byte limit {limit}")]
    ByteLimitExceeded {
        /// Byte count attempted.
        attempted: u64,
        /// Configured byte quota.
        limit: u64,
    },
    /// Active entries would exceed the configured entry limit.
    #[error("entry count {attempted} exceeds entry limit {limit}")]
    EntryLimitExceeded {
        /// Entry count attempted.
        attempted: u64,
        /// Configured entry quota.
        limit: u64,
    },
    /// Active associations would exceed the configured association limit.
    #[error("association count {attempted} exceeds association limit {limit}")]
    AssociationLimitExceeded {
        /// Association count attempted.
        attempted: u64,
        /// Configured association quota.
        limit: u64,
    },
    /// An arithmetic operation overflowed `u64`.
    #[error("ledger arithmetic overflow on {field}")]
    ArithmeticOverflow {
        /// Counter field that overflowed.
        field: &'static str,
    },
    /// An eviction or adjustment attempted to subtract more bytes/rows than currently tracked.
    #[error("ledger underflow on {field}: tried to subtract {delta} from {current}")]
    Underflow {
        /// Counter field that underflowed.
        field: &'static str,
        /// Value before subtraction.
        current: u64,
        /// Subtracted delta.
        delta: u64,
    },
}
