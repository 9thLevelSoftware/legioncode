//! Tests and `proptest` suites for `backoff`, `probe`, and `dpop_renewal`.

use proptest::prelude::*;
use t3_client_core::backoff::{
    APPROVAL_PENDING_FAST_RETRIES, APPROVAL_PENDING_RETRY_DELAY_MS, BACKOFF_RESET_AFTER_MS,
    FailureReason, RETRY_BASE_DELAY_MS, RETRY_MAX_DELAY_MS, ReconnectBackoff, RouteCooldownTracker,
    SESSION_LOSS_RETRY_DELAY_MS, SilentRouteBackoff, jittered_retry_delay_ms,
    retry_delay_ceiling_ms, session_loss_retry_delay_ms,
};
use t3_client_core::dpop_renewal::{
    DPOP_ACCESS_TOKEN_REFRESH_SKEW_MS, DPOP_RENEWAL_POST_EXPIRY_DIRECT_FLOOR_MS,
    DPOP_RENEWAL_PRE_EXPIRY_FLOOR_MS, DpopRenewalTracker, next_renewal_delay_ms,
};
use t3_client_core::probe::{
    CONNECTION_PROBE_TIMEOUT_MS, ConnectionRouteKind, ConnectionWakeup,
    LONG_RESUME_PROBE_TIMEOUT_MS, NetworkStatus, ProbeAction, ProbeEndReason, ProbeSignal,
    QUICK_CONNECTION_PROBE_TIMEOUT_MS, SilentSocketStateMachine, probe_timeout_for,
};

#[test]
fn approval_pending_holds_4s_for_150_waits_without_advancing_backoff() {
    let mut backoff = ReconnectBackoff::new();
    for step in 1..=APPROVAL_PENDING_FAST_RETRIES {
        let decision = backoff.record_failure(FailureReason::ApprovalPending, false, 0.99);
        assert_eq!(decision.delay_ms, APPROVAL_PENDING_RETRY_DELAY_MS);
        assert_eq!(decision.attempt, step);
        assert!(decision.awaiting_approval);
        assert_eq!(backoff.backoff_failures(), 0);
    }
    // Attempt 151 exceeds APPROVAL_PENDING_FAST_RETRIES and keeps delay >= 4_000 ms.
    let expired = backoff.record_failure(FailureReason::ApprovalPending, false, 0.0);
    assert!(!expired.awaiting_approval);
    assert!(expired.delay_ms >= APPROVAL_PENDING_RETRY_DELAY_MS);
    assert_eq!(backoff.backoff_failures(), 1);

    // An outage right after fast waits starts at the bottom of the exponential ladder.
    let mut after_waits = ReconnectBackoff::new();
    for _ in 0..8 {
        let d = after_waits.record_failure(FailureReason::ApprovalPending, false, 1.0);
        assert_eq!(d.delay_ms, APPROVAL_PENDING_RETRY_DELAY_MS);
    }
    let outage = after_waits.record_failure(FailureReason::Transient, false, 1.0);
    assert_eq!(outage.delay_ms, 2_000);
}

#[test]
fn backoff_resets_only_after_stable_connection_and_tracks_route_cooldowns() {
    let mut backoff = ReconnectBackoff::new();
    let first_loss = backoff.record_failure(FailureReason::Transient, true, 0.5);
    assert!(first_loss.first_loss);
    assert!((125..=SESSION_LOSS_RETRY_DELAY_MS).contains(&first_loss.delay_ms));

    backoff.on_session_ended(BACKOFF_RESET_AFTER_MS - 1);
    assert_eq!(backoff.failure_count(), 1);

    backoff.on_session_ended(BACKOFF_RESET_AFTER_MS);
    assert_eq!(backoff.failure_count(), 0);

    let mut cooldowns = RouteCooldownTracker::new();
    cooldowns.record_failed_switch("lan-1", 10_000);
    assert!(cooldowns.is_cooled_down("lan-1", 309_999));
    assert!(!cooldowns.is_cooled_down("lan-1", 310_000));

    let mut silent = SilentRouteBackoff::new();
    let gen0 = silent.generation();
    assert!(silent.record_silent("lan-1", gen0, 1_000));
    assert!(silent.is_backed_off("lan-1", 120_999));
    assert!(!silent.is_backed_off("lan-1", 121_000));
    silent.forget_all();
    assert!(!silent.record_silent("lan-1", gen0, 2_000));
    assert!(!silent.is_backed_off("lan-1", 2_000));
}

#[test]
fn probe_timeouts_match_supervisor_matrix() {
    assert_eq!(
        probe_timeout_for(
            ProbeSignal::Wakeup(ConnectionWakeup::ApplicationActiveLong),
            Some(ConnectionRouteKind::Relay)
        ),
        Some(LONG_RESUME_PROBE_TIMEOUT_MS)
    );
    assert_eq!(
        probe_timeout_for(
            ProbeSignal::Wakeup(ConnectionWakeup::ApplicationActiveProbe),
            Some(ConnectionRouteKind::Loopback)
        ),
        Some(QUICK_CONNECTION_PROBE_TIMEOUT_MS)
    );
    assert_eq!(
        probe_timeout_for(
            ProbeSignal::Wakeup(ConnectionWakeup::ApplicationActiveProbe),
            Some(ConnectionRouteKind::Lan)
        ),
        Some(QUICK_CONNECTION_PROBE_TIMEOUT_MS)
    );
    assert_eq!(
        probe_timeout_for(
            ProbeSignal::RetryRequested,
            Some(ConnectionRouteKind::Relay)
        ),
        Some(QUICK_CONNECTION_PROBE_TIMEOUT_MS)
    );
    assert_eq!(
        probe_timeout_for(
            ProbeSignal::NetworkChanged(NetworkStatus::Offline),
            Some(ConnectionRouteKind::Relay)
        ),
        Some(QUICK_CONNECTION_PROBE_TIMEOUT_MS)
    );
    assert_eq!(
        probe_timeout_for(
            ProbeSignal::Wakeup(ConnectionWakeup::ApplicationActive),
            Some(ConnectionRouteKind::Lan)
        ),
        Some(CONNECTION_PROBE_TIMEOUT_MS)
    );
    assert_eq!(
        probe_timeout_for(
            ProbeSignal::Wakeup(ConnectionWakeup::ApplicationActiveProbe),
            Some(ConnectionRouteKind::Relay)
        ),
        Some(CONNECTION_PROBE_TIMEOUT_MS)
    );
    assert_eq!(
        probe_timeout_for(
            ProbeSignal::Wakeup(ConnectionWakeup::ApplicationFocused),
            Some(ConnectionRouteKind::Lan)
        ),
        None
    );
}

#[test]
fn silent_socket_machine_enforces_poll_recheck_remote_floor_and_foreground_miss() {
    // 1) LAN host failing first check rechecks after 1_000 ms (500 + 1_000 = 1_500).
    let mut lan = SilentSocketStateMachine::new(Some(ConnectionRouteKind::Lan));
    assert_eq!(
        lan.on_signal(
            ProbeSignal::Wakeup(ConnectionWakeup::ApplicationActiveProbe),
            0
        ),
        vec![ProbeAction::StartProbe]
    );
    assert_eq!(lan.on_tick(500, 0), vec![ProbeAction::LoadInitialConfig]);
    assert_eq!(
        lan.on_config_loaded(true, 0, 500),
        vec![ProbeAction::CheckHost]
    );
    assert_eq!(lan.on_host_check_result(false, 0, 500), Vec::new());
    assert_eq!(lan.on_tick(1_499, 0), vec![ProbeAction::PollInboundFrames]);
    assert_eq!(lan.on_tick(1_500, 0), vec![ProbeAction::CheckHost]);
    assert_eq!(
        lan.on_host_check_result(true, 0, 1_500),
        vec![ProbeAction::ReplaceSocket {
            reason: ProbeEndReason::SilentSocket,
        }]
    );

    // 2) Remote long resume skips 2_500ms floor, and frame arrival extends deadline to 3_000ms.
    let mut remote_long = SilentSocketStateMachine::new(Some(ConnectionRouteKind::Relay));
    let _ = remote_long.on_signal(
        ProbeSignal::Wakeup(ConnectionWakeup::ApplicationActiveLong),
        0,
    );
    assert_eq!(
        remote_long.on_tick(500, 10),
        vec![ProbeAction::LoadInitialConfig]
    );
    assert_eq!(
        remote_long.on_config_loaded(true, 10, 500),
        vec![ProbeAction::CheckHost]
    );
    remote_long.on_frames_observed(11, 700);
    // Extended to 3_000ms instead of timing out at 1_500ms.
    assert!(
        !remote_long
            .on_tick(1_500, 11)
            .contains(&ProbeAction::ReplaceSocket {
                reason: ProbeEndReason::LongResumeTimeout
            })
    );
    assert_eq!(
        remote_long.on_tick(3_000, 11),
        vec![ProbeAction::ReplaceSocket {
            reason: ProbeEndReason::LongResumeTimeout
        }]
    );

    // 3) Foreground probe leaves host alone and tolerates one 15s miss before timing out at 30s.
    let mut fg = SilentSocketStateMachine::new(Some(ConnectionRouteKind::Lan));
    assert_eq!(
        fg.on_signal(ProbeSignal::Wakeup(ConnectionWakeup::ApplicationActive), 0),
        vec![ProbeAction::StartProbe]
    );
    assert_eq!(fg.on_tick(15_000, 0), vec![ProbeAction::StartProbe]);
    assert_eq!(fg.on_tick(30_000, 0), vec![ProbeAction::ProbeTimedOut]);
}

#[test]
fn silent_socket_machine_rechecks_after_frame_when_network_changed() {
    let mut machine = SilentSocketStateMachine::new(Some(ConnectionRouteKind::Relay));
    let _ = machine.on_signal(
        ProbeSignal::Wakeup(ConnectionWakeup::ApplicationActiveProbe),
        0,
    );
    let _ = machine.on_tick(500, 0);
    let _ = machine.on_config_loaded(true, 0, 500);
    let _ = machine.on_signal(ProbeSignal::Wakeup(ConnectionWakeup::NetworkChanged), 1_000);
    // Poll at 1_600 observes frame 1 -> starts fresh check at 1_600 -> floor at 1_600 + 2_500 = 4_100.
    let _ = machine.on_tick(1_600, 1);
    assert_eq!(
        machine.on_tick(2_100, 1),
        vec![ProbeAction::LoadInitialConfig]
    );
    assert_eq!(
        machine.on_config_loaded(true, 1, 2_100),
        vec![ProbeAction::CheckHost]
    );
    assert_eq!(machine.on_host_check_result(true, 1, 2_100), Vec::new());
    assert_eq!(
        machine.on_tick(4_099, 1),
        vec![ProbeAction::PollInboundFrames]
    );
    assert_eq!(
        machine.on_tick(4_100, 1),
        vec![ProbeAction::ReplaceSocket {
            reason: ProbeEndReason::SilentSocket
        }]
    );
}

#[test]
fn dpop_renewal_delays_match_relay_and_direct_schedules() {
    assert_eq!(next_renewal_delay_ms(3_600_000, 0, false), Some(3_540_000));
    assert_eq!(
        next_renewal_delay_ms(3_600_000, 3_540_000, false),
        Some(DPOP_RENEWAL_PRE_EXPIRY_FLOOR_MS)
    );
    assert_eq!(next_renewal_delay_ms(1_000, 5_000, false), None);
    assert_eq!(
        next_renewal_delay_ms(1_000, 5_000, true),
        Some(DPOP_RENEWAL_POST_EXPIRY_DIRECT_FLOOR_MS)
    );

    let mut tracker = DpopRenewalTracker::new(3_600_000, false);
    assert_eq!(tracker.next_delay_ms(0), Some(3_540_000));
    tracker.record_transient_failure();
    assert_eq!(tracker.next_delay_ms(3_540_000), Some(5_000));
    tracker.record_renewed(7_145_000);
    assert_eq!(tracker.next_delay_ms(3_545_000), Some(3_540_000));
    tracker.record_blocked_failure();
    assert_eq!(tracker.next_delay_ms(3_545_000), None);
}

proptest! {
    #[test]
    fn prop_backoff_stays_within_exact_jitter_and_cap_bounds(
        idx in 0_u32..64,
        random in -0.5_f64..1.5_f64,
    ) {
        let ceiling = retry_delay_ceiling_ms(idx);
        prop_assert!(ceiling >= RETRY_BASE_DELAY_MS * 2);
        prop_assert!(ceiling <= RETRY_MAX_DELAY_MS);
        let delay = jittered_retry_delay_ms(idx, random);
        prop_assert!(delay >= ceiling / 2);
        prop_assert!(delay <= ceiling);

        let loss = session_loss_retry_delay_ms(random);
        prop_assert!((125..=SESSION_LOSS_RETRY_DELAY_MS).contains(&loss));
    }

    #[test]
    fn prop_probe_deadlines_never_exceed_15s_and_only_known_values(
        route_idx in 0_usize..8,
        signal_idx in 0_usize..10,
    ) {
        let route = match route_idx {
            0 => Some(ConnectionRouteKind::Relay),
            1 => Some(ConnectionRouteKind::Loopback),
            2 => Some(ConnectionRouteKind::Lan),
            3 => Some(ConnectionRouteKind::Tailnet),
            4 => Some(ConnectionRouteKind::Vpn),
            5 => Some(ConnectionRouteKind::Public),
            6 => Some(ConnectionRouteKind::Ssh),
            _ => None,
        };
        let signal = match signal_idx {
            0 => ProbeSignal::ConnectRequested,
            1 => ProbeSignal::DisconnectRequested,
            2 => ProbeSignal::RetryRequested,
            3 => ProbeSignal::NetworkChanged(NetworkStatus::Offline),
            4 => ProbeSignal::NetworkChanged(NetworkStatus::Online),
            5 => ProbeSignal::Wakeup(ConnectionWakeup::ApplicationActive),
            6 => ProbeSignal::Wakeup(ConnectionWakeup::ApplicationActiveProbe),
            7 => ProbeSignal::Wakeup(ConnectionWakeup::ApplicationActiveLong),
            8 => ProbeSignal::Wakeup(ConnectionWakeup::NetworkChanged),
            _ => ProbeSignal::Wakeup(ConnectionWakeup::ApplicationFocused),
        };
        if let Some(ms) = probe_timeout_for(signal, route) {
            prop_assert!(
                ms == LONG_RESUME_PROBE_TIMEOUT_MS
                    || ms == QUICK_CONNECTION_PROBE_TIMEOUT_MS
                    || ms == CONNECTION_PROBE_TIMEOUT_MS
            );
            prop_assert!(ms <= CONNECTION_PROBE_TIMEOUT_MS);
        }
    }

    #[test]
    fn prop_dpop_renewal_respects_floors_and_expiry_contract(
        expires_at in 0_u64..10_000_000,
        now in 0_u64..10_000_000,
        is_direct in any::<bool>(),
    ) {
        let delay = next_renewal_delay_ms(expires_at, now, is_direct);
        if !is_direct && now >= expires_at {
            prop_assert_eq!(delay, None);
        } else {
            let ms = delay.expect("relay pre-expiry or direct must return Some");
            if now < expires_at {
                prop_assert!(ms >= DPOP_RENEWAL_PRE_EXPIRY_FLOOR_MS);
                let expected_target = expires_at.saturating_sub(DPOP_ACCESS_TOKEN_REFRESH_SKEW_MS);
                prop_assert_eq!(ms, expected_target.saturating_sub(now).max(DPOP_RENEWAL_PRE_EXPIRY_FLOOR_MS));
            } else {
                prop_assert_eq!(ms, DPOP_RENEWAL_POST_EXPIRY_DIRECT_FLOOR_MS);
            }
        }
    }
}
