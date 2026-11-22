//! Tests for `legion_edge::task_cache` (`BUG-30`, admission, D1 batch ledger, and OIDC cooldown).

use legion_edge::task_cache::{
    AdmissionKind, CacheAdmissionPolicy, CompletedPart, D1BatchError, D1BatchLedger, D1LedgerOp,
    DeploymentQuotaState, MAX_MULTIPART_PARTS, OIDC_JWKS_COOLDOWN_MS, OidcCooldownError,
    OidcCooldownTracker, PartValidationError, SavingsKey, ScopeQuotaState, SummaryRecord,
    TaskSavingsEntry, TaskSavingsSource, WriteQuotaCheckInput, WriteQuotaError, parse_part_number,
    validate_complete_parts, validate_part_number,
};
use proptest::prelude::*;

#[test]
fn bug30_part_numbers_zero_and_above_64_rejected_at_put() -> Result<(), String> {
    for part in 1..=MAX_MULTIPART_PARTS {
        assert_eq!(validate_part_number(part), Ok(part));
    }

    for invalid in [0_u32, 65, 99, 100, u32::MAX] {
        let err = validate_part_number(invalid)
            .err()
            .ok_or_else(|| format!("expected part {invalid} to fail"))?;
        assert_eq!(err.http_status(), 400);
        assert_eq!(err.error_code(), "invalid_part_number");
        assert_eq!(
            err,
            PartValidationError::InvalidPartNumber {
                part: invalid,
                max_parts: MAX_MULTIPART_PARTS,
            }
        );
    }

    assert_eq!(parse_part_number("64"), Ok(64));
    for raw in ["0", "01", "65", "99", "100", "-1", "abc", ""] {
        let err = parse_part_number(raw)
            .err()
            .ok_or_else(|| format!("expected raw '{raw}' to fail"))?;
        assert_eq!(err.http_status(), 400);
        assert_eq!(err.error_code(), "invalid_part_number");
    }

    let valid_parts = vec![
        CompletedPart {
            part_number: 1,
            etag: "e1".to_owned(),
        },
        CompletedPart {
            part_number: 64,
            etag: "e64".to_owned(),
        },
    ];
    validate_complete_parts(&valid_parts).map_err(|e| e.to_string())?;
    assert!(validate_complete_parts(&[]).is_err());
    assert!(
        validate_complete_parts(&[
            CompletedPart {
                part_number: 1,
                etag: "a".to_owned()
            },
            CompletedPart {
                part_number: 1,
                etag: "b".to_owned()
            },
        ])
        .is_err()
    );
    Ok(())
}

#[test]
fn admission_policy_bounds_concurrency_and_write_quotas() -> Result<(), String> {
    let mut policy = CacheAdmissionPolicy::new();
    let mut s1 = policy
        .try_acquire(AdmissionKind::Store)
        .map_err(|e| e.to_string())?;
    let mut s2 = policy
        .try_acquire(AdmissionKind::Store)
        .map_err(|e| e.to_string())?;
    let store_rej = policy
        .try_acquire(AdmissionKind::Store)
        .err()
        .ok_or("expected rejection")?;
    assert_eq!(store_rej.http_status(), 503);
    assert_eq!(store_rej.error_code(), "concurrency_limit");

    let mut r1 = policy
        .try_acquire(AdmissionKind::Read)
        .map_err(|e| e.to_string())?;
    let mut r2 = policy
        .try_acquire(AdmissionKind::Read)
        .map_err(|e| e.to_string())?;
    let mut r3 = policy
        .try_acquire(AdmissionKind::Read)
        .map_err(|e| e.to_string())?;
    let mut r4 = policy
        .try_acquire(AdmissionKind::Read)
        .map_err(|e| e.to_string())?;
    assert!(policy.try_acquire(AdmissionKind::Read).is_err());

    assert!(policy.release(&mut s1));
    assert!(!policy.release(&mut s1));
    assert!(policy.release(&mut s2));
    assert_eq!(policy.active_stores(), 0);
    for r in [&mut r1, &mut r2, &mut r3, &mut r4] {
        assert!(policy.release(r));
    }
    assert_eq!(policy.active_reads(), 0);

    let base = WriteQuotaCheckInput {
        deployment: DeploymentQuotaState {
            enabled: true,
            writes_enabled: true,
            charged_bytes: 100,
            byte_limit: 500,
        },
        scope: ScopeQuotaState {
            enabled: true,
            writes_enabled: true,
            charged_bytes: 50,
            byte_limit: 300,
            policy_version: 2,
        },
        expected_policy_version: 2,
        token_exp_secs: 1_000,
    };
    CacheAdmissionPolicy::check_write_quota(&base, 250, 999).map_err(|e| e.to_string())?;
    assert_eq!(
        CacheAdmissionPolicy::check_write_quota(&base, 251, 999),
        Err(WriteQuotaError::ByteQuotaExceeded)
    );
    assert_eq!(
        CacheAdmissionPolicy::check_write_quota(&base, 10, 1_000),
        Err(WriteQuotaError::TokenExpired)
    );
    Ok(())
}

#[test]
fn oidc_cooldown_enforces_30s_even_from_t0() -> Result<(), String> {
    let mut tracker = OidcCooldownTracker::new();
    tracker.try_start_refresh(0).map_err(|e| e.to_string())?;
    assert_eq!(tracker.last_attempt_ms(), Some(0));
    assert_eq!(
        tracker.try_start_refresh(OIDC_JWKS_COOLDOWN_MS - 1),
        Err(OidcCooldownError { retry_after_ms: 1 })
    );
    tracker
        .try_start_refresh(OIDC_JWKS_COOLDOWN_MS)
        .map_err(|e| e.to_string())?;
    assert_eq!(tracker.last_attempt_ms(), Some(OIDC_JWKS_COOLDOWN_MS));
    Ok(())
}

#[test]
fn d1_batch_ledger_atomic_accounting_dedup_and_rollback() -> Result<(), String> {
    let mut ledger = D1BatchLedger::new(1_000, 10, 10);
    let summary = SummaryRecord {
        id: "a".repeat(64),
        recorded_at_ms: 5_000,
        developer: "alice".to_owned(),
        host: "mbp".to_owned(),
        platform: "darwin".to_owned(),
        arch: "arm64".to_owned(),
        ci: false,
        tasks: vec![
            TaskSavingsEntry {
                package: "server".to_owned(),
                task: "check".to_owned(),
                source: TaskSavingsSource::Remote,
                saved_ms: 900,
            },
            TaskSavingsEntry {
                package: "web".to_owned(),
                task: "build".to_owned(),
                source: TaskSavingsSource::Local,
                saved_ms: 400,
            },
        ],
    };

    let receipt = ledger
        .execute_batch(&[
            D1LedgerOp::Reserve { bytes: 600 },
            D1LedgerOp::Publish {
                key_is_new: true,
                association_is_new: true,
                actual_bytes: 500,
                reserved_bytes: 600,
                retired_bytes: None,
            },
            D1LedgerOp::RecordHit {
                fallback: false,
                rows_read: 2,
            },
            D1LedgerOp::RecordMiss { rows_read: 3 },
            D1LedgerOp::RecordSavings {
                summary: summary.clone(),
            },
            // Duplicate receipt ID inside same or later batch is ignored atomically.
            D1LedgerOp::RecordSavings { summary },
        ])
        .map_err(|e| e.to_string())?;
    assert_eq!(receipt.savings_receipts_inserted, 1);
    assert_eq!(ledger.charged_bytes(), 500);
    assert_eq!(ledger.entry_count(), 1);
    assert_eq!(ledger.association_count(), 1);
    assert_eq!(ledger.hits(), 1);
    assert_eq!(ledger.misses(), 1);

    let key = SavingsKey {
        developer: "alice".to_owned(),
        host: "mbp".to_owned(),
        platform: "darwin".to_owned(),
        arch: "arm64".to_owned(),
        ci: false,
    };
    let totals = ledger.savings_for(&key).ok_or("missing savings row")?;
    assert_eq!(totals.remote_ms, 900);
    assert_eq!(totals.local_ms, 400);

    // Failed batch (RecordHit followed by ByteLimitExceeded) rolls back completely!
    let snapshot = ledger.clone();
    let err = ledger
        .execute_batch(&[
            D1LedgerOp::RecordHit {
                fallback: true,
                rows_read: 4,
            },
            D1LedgerOp::Reserve { bytes: 600 },
        ])
        .err()
        .ok_or("expected byte limit failure")?;
    assert_eq!(
        err,
        D1BatchError::ByteLimitExceeded {
            attempted: 1_100,
            limit: 1_000,
        }
    );
    assert_eq!(ledger, snapshot);

    // Eviction releases bytes & entry counts cleanly.
    ledger
        .execute_batch(&[D1LedgerOp::Evict {
            bytes: 500,
            entries: 1,
            associations: 1,
        }])
        .map_err(|e| e.to_string())?;
    assert_eq!(ledger.charged_bytes(), 0);
    assert_eq!(ledger.entry_count(), 0);
    assert_eq!(ledger.evictions(), 1);
    assert_eq!(ledger.evicted_bytes(), 500);
    Ok(())
}

proptest! {
    #[test]
    fn proptest_part_number_validation_exact_range(part in any::<u32>()) {
        let res = validate_part_number(part);
        prop_assert_eq!(res.is_ok(), (1..=MAX_MULTIPART_PARTS).contains(&part));
    }

    #[test]
    fn proptest_d1_ledger_never_exceeds_limits_on_failed_batches(
        reserves in prop::collection::vec(1_u64..800, 1..20),
    ) {
        let mut ledger = D1BatchLedger::new(1_000, 10, 10);
        for bytes in reserves {
            let before = ledger.clone();
            let res = ledger.execute_batch(&[
                D1LedgerOp::RecordHit { fallback: false, rows_read: 1 },
                D1LedgerOp::Reserve { bytes },
            ]);
            if res.is_err() {
                prop_assert_eq!(&ledger, &before);
            }
            prop_assert!(ledger.charged_bytes() <= 1_000);
        }
    }
}
