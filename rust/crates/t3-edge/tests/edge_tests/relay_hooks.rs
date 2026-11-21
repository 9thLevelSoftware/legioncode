//! Tests for `t3_edge::relay_hooks` (`BUG-29` and `BUG-32`).

use proptest::prelude::*;
use t3_edge::relay_hooks::{
    CLOUDFLARE_MAX_RATE_LIMIT_PAUSE_MS, CLOUDFLARE_PAUSE_REFRESH_MS, CloudflareApiPauseGate,
    CloudflareApiPaused, DeliveryOutcome, DeliveryRecordOutcome, HOOK_INBOX_MAX_BYTES,
    HOOK_INBOX_MAX_PER_HOOK, HOOK_INBOX_TTL_MS, HeldHook, HeldHooksTarget, HoldRefusalReason,
    HookInboxState, MAX_COUNTED_FAILURES, MAX_RETRY_DELAY_MS, TUNNEL_OFFLINE_STATUS,
    TargetOutcomeResult, retry_delay_ms,
};

#[test]
fn bug29_wake_increments_generation_and_ignores_stale_failed_8s_sends() -> Result<(), String> {
    let mut inbox = HookInboxState::new();
    inbox
        .hold(
            HeldHook::new("del-1", "hook-a", 1_000, "POST", b"hello".to_vec()),
            "https://old.tunnel.example",
            1_000,
        )
        .map_err(|e| e.to_string())?;
    let batch1 = inbox
        .begin_delivery_batch(1_000)
        .ok_or("expected batch 1")?;
    assert_eq!(batch1.target_generation, 1);
    assert_eq!(batch1.base_url, "https://old.tunnel.example");

    // Wake arrives on a new tunnel URL while the 8s upstream send is still in flight.
    assert!(inbox.wake("https://new.tunnel.example"));
    let after_wake = inbox.target().ok_or("target missing after wake")?;
    assert_eq!(after_wake.target_generation, 2);
    assert_eq!(after_wake.failures, 0);
    assert_eq!(after_wake.base_url, "https://new.tunnel.example");

    // Stale 8s timeout / 530 / 502 from generation 1 must be ignored (BUG-29).
    let stale_unreachable = inbox.record_delivery_outcome(
        batch1.target_generation,
        "del-1",
        DeliveryOutcome::Unreachable,
        9_000,
    );
    assert_eq!(
        stale_unreachable,
        DeliveryRecordOutcome::IgnoredStale {
            snapshot_generation: 1,
            current_generation: 2,
            next_attempt_at_ms: Some(9_000),
        }
    );
    assert_eq!(inbox.pending_count(), 1);
    assert_eq!(inbox.target().map(|t| t.failures), Some(0));

    // Even a stale Busy or Delivered outcome from generation 1 is ignored.
    let stale_delivered = inbox.record_delivery_outcome(
        batch1.target_generation,
        "del-1",
        DeliveryOutcome::Delivered,
        9_001,
    );
    assert!(stale_delivered.is_ignored_stale());
    assert_eq!(inbox.pending_count(), 1);

    // Next batch uses generation 2 and delivers cleanly.
    let batch2 = inbox
        .begin_delivery_batch(9_002)
        .ok_or("expected batch 2")?;
    assert_eq!(batch2.target_generation, 2);
    assert_eq!(batch2.base_url, "https://new.tunnel.example");
    assert_eq!(
        inbox.record_delivery_outcome(
            batch2.target_generation,
            "del-1",
            DeliveryOutcome::Delivered,
            9_050,
        ),
        DeliveryRecordOutcome::Delivered {
            remaining_pending: false,
        }
    );
    assert_eq!(inbox.pending_count(), 0);
    Ok(())
}

#[test]
fn hook_inbox_caps_ttl_retry_schedule_and_clear_semantics() -> Result<(), String> {
    assert_eq!(retry_delay_ms(0), 10_000);
    assert_eq!(retry_delay_ms(1), 10_000);
    assert_eq!(retry_delay_ms(18), 10_000);
    assert_eq!(retry_delay_ms(19), 30_000);
    assert_eq!(retry_delay_ms(20), 60_000);
    assert_eq!(retry_delay_ms(23), 480_000);
    assert_eq!(retry_delay_ms(24), MAX_RETRY_DELAY_MS);
    assert_eq!(retry_delay_ms(99), MAX_RETRY_DELAY_MS);

    assert_eq!(
        DeliveryOutcome::from_http_status(TUNNEL_OFFLINE_STATUS),
        DeliveryOutcome::Unreachable
    );
    assert_eq!(
        DeliveryOutcome::from_http_status(429),
        DeliveryOutcome::Busy
    );
    assert_eq!(
        DeliveryOutcome::from_http_status(200),
        DeliveryOutcome::Delivered
    );
    assert_eq!(
        DeliveryOutcome::from_send_result(&Err::<u16, _>("timeout")),
        DeliveryOutcome::Unreachable
    );

    let mut inbox = HookInboxState::new();
    for idx in 0..HOOK_INBOX_MAX_PER_HOOK {
        inbox
            .hold(
                HeldHook::new(format!("h-{idx}"), "hook-cap", 1_000, "POST", vec![1]),
                "https://env.test",
                1_000,
            )
            .map_err(|e| e.to_string())?;
    }
    assert_eq!(
        inbox.hold(
            HeldHook::new("h-0", "hook-cap", 1_000, "POST", vec![1]),
            "https://env.test",
            1_000,
        ),
        Err(HoldRefusalReason::AlreadyHeld)
    );
    assert_eq!(
        inbox.hold(
            HeldHook::new("h-overflow", "hook-cap", 1_000, "POST", vec![1]),
            "https://env.test",
            1_000,
        ),
        Err(HoldRefusalReason::MaxPerHook)
    );
    assert_eq!(
        inbox.hold(
            HeldHook::new(
                "too-big",
                "other-hook",
                1_000,
                "POST",
                vec![0; HOOK_INBOX_MAX_BYTES],
            ),
            "https://env.test",
            1_000,
        ),
        Err(HoldRefusalReason::MaxBytes)
    );

    // Expiry removes items strictly older than 24 hours.
    assert_eq!(inbox.prune_expired(1_000 + HOOK_INBOX_TTL_MS), 0);
    assert_eq!(
        inbox.prune_expired(1_000 + HOOK_INBOX_TTL_MS + 1),
        HOOK_INBOX_MAX_PER_HOOK
    );
    assert_eq!(inbox.pending_bytes(), 0);

    // Clearing while in flight causes outcome to be ignored without scheduling retries.
    inbox
        .hold(
            HeldHook::new("c-1", "k", 2_000, "POST", b"x".to_vec()),
            "https://env.test",
            2_000,
        )
        .map_err(|e| e.to_string())?;
    let batch = inbox.begin_delivery_batch(2_000).ok_or("missing batch")?;
    inbox.clear();
    assert_eq!(
        inbox.record_delivery_outcome(
            batch.target_generation,
            "c-1",
            DeliveryOutcome::Unreachable,
            2_500,
        ),
        DeliveryRecordOutcome::IgnoredCleared
    );
    Ok(())
}

#[test]
fn bug32_cloudflare_api_pause_gate_never_advances_checked_at_on_err() -> Result<(), String> {
    let mut gate = CloudflareApiPauseGate::new();
    assert!(gate.should_refresh(1_000));

    // Transient shared DB read error fails open AND keeps checked_at_ms = None (BUG-32).
    let pass1 = gate
        .evaluate(1_000, || Err::<u64, _>("d1 transient timeout"))
        .map_err(|e| e.to_string())?;
    assert!(!pass1.refreshed_from_shared);
    assert_eq!(pass1.shared_read_error, Some("d1 transient timeout"));
    assert_eq!(gate.checked_at_ms(), None);
    assert!(gate.should_refresh(1_001));

    // Subsequent Ok(0) advances checked_at_ms to 1_100 and suppresses reads for 5_000 ms.
    let pass2 = gate
        .evaluate(1_100, || Ok::<u64, &str>(0))
        .map_err(|e| e.to_string())?;
    assert!(pass2.refreshed_from_shared);
    assert_eq!(gate.checked_at_ms(), Some(1_100));
    assert!(!gate.should_refresh(1_100 + CLOUDFLARE_PAUSE_REFRESH_MS - 1));
    assert!(gate.should_refresh(1_100 + CLOUDFLARE_PAUSE_REFRESH_MS));

    // Shared pause into the future blocks calls without hitting Cloudflare.
    let rejected = gate
        .evaluate(6_100, || Ok::<u64, &str>(15_000))
        .expect_err("active pause must reject");
    assert_eq!(
        rejected,
        CloudflareApiPaused {
            retry_after_ms: 8_900
        }
    );
    assert_eq!(gate.checked_at_ms(), Some(6_100));

    // Once pause expires at 15_000, an Err read does NOT advance checked_at_ms beyond 6_100.
    assert!(
        gate.apply_shared_pause_read::<&str>(15_000, Err("err"))
            .is_err()
    );
    assert_eq!(gate.checked_at_ms(), Some(6_100));
    assert!(gate.should_refresh(15_001));

    // Local 429 clamps retry-after to 120_000 ms.
    let clamped_until = gate.record_rate_limit(20_000, Some(999_999));
    assert_eq!(clamped_until, 20_000 + CLOUDFLARE_MAX_RATE_LIMIT_PAUSE_MS);
    Ok(())
}

proptest! {
    #[test]
    fn proptest_target_generation_fences_stale_failures(
        wakes in 1_u8..12,
        stale_outcomes in prop::collection::vec(0_u8..3, 1..20),
    ) {
        let mut target = HeldHooksTarget::new("https://0.tunnel.test");
        let initial_gen = target.target_generation;
        for step in 1..=wakes {
            let new_gen = target.wake(format!("https://{step}.tunnel.test"));
            prop_assert!(new_gen > initial_gen);
            prop_assert_eq!(target.failures, 0);
        }
        let current_gen = target.target_generation;
        for code in stale_outcomes {
            let outcome = match code {
                0 => DeliveryOutcome::Delivered,
                1 => DeliveryOutcome::Busy,
                _ => DeliveryOutcome::Unreachable,
            };
            let res = target.record_outcome(current_gen - 1, outcome);
            prop_assert_eq!(res, TargetOutcomeResult::IgnoredStale { current_generation: current_gen });
            prop_assert_eq!(target.failures, 0);
        }
        for _ in 0..40 {
            let _ = target.record_outcome(current_gen, DeliveryOutcome::Unreachable);
            prop_assert!(target.failures <= MAX_COUNTED_FAILURES);
        }
    }

    #[test]
    fn proptest_pause_gate_err_never_advances_checked_at(
        reads in prop::collection::vec((1_u64..10_000, any::<bool>(), 0_u64..20_000), 1..25),
    ) {
        let mut gate = CloudflareApiPauseGate::new();
        let mut now = 0_u64;
        for (delta, is_ok, shared_until) in reads {
            now = now.saturating_add(delta);
            let prev_checked = gate.checked_at_ms();
            if is_ok {
                let _ = gate.apply_shared_pause_read::<&str>(now, Ok(shared_until));
                prop_assert_eq!(gate.checked_at_ms(), Some(now));
            } else {
                let _ = gate.apply_shared_pause_read::<&str>(now, Err("fail"));
                prop_assert_eq!(gate.checked_at_ms(), prev_checked);
            }
        }
    }
}
