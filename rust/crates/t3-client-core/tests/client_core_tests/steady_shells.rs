//! Tests and `proptest` suites for `steady_shells` and BUG-21 isolation.

use proptest::prelude::*;
use t3_client_core::steady_shells::{
    EnvironmentShellStatus, STEADY_SHELL_RELEASE_WINDOW_MS, SteadyShellCoordinator,
    SteadyShellInput,
};
use t3_contracts::ids::EnvironmentId;

fn env(id: &str) -> EnvironmentId {
    EnvironmentId::new(id).expect("valid env id")
}

fn shell(
    environment_id: EnvironmentId,
    snapshot: Option<&'static str>,
    seq: Option<u64>,
    status: EnvironmentShellStatus,
    generation: u64,
    connecting: bool,
    row_actions: u64,
) -> SteadyShellInput<&'static str> {
    SteadyShellInput::new(
        environment_id,
        snapshot,
        seq,
        status,
        generation,
        connecting,
        row_actions,
    )
}

#[test]
fn steady_shells_holds_peers_for_2s_and_releases_on_local_action_or_caught_up() {
    let a = env("a");
    let b = env("b");
    let mut coord = SteadyShellCoordinator::new(STEADY_SHELL_RELEASE_WINDOW_MS);
    coord.mount();

    let s0 = coord.step(
        &[
            shell(
                a.clone(),
                Some("cached-a"),
                Some(1),
                EnvironmentShellStatus::Synchronizing,
                0,
                true,
                0,
            ),
            shell(
                b.clone(),
                Some("cached-b"),
                Some(2),
                EnvironmentShellStatus::Synchronizing,
                0,
                true,
                0,
            ),
        ],
        0,
    );
    assert!(s0.changed);
    assert_eq!(s0.release_at_ms, None);

    // Local snapshot enrichment (same sequence, same generation) releases immediately.
    let local = coord.step(
        &[
            shell(
                a.clone(),
                Some("cached-a-links"),
                Some(1),
                EnvironmentShellStatus::Synchronizing,
                0,
                true,
                0,
            ),
            shell(
                b.clone(),
                Some("cached-b"),
                Some(2),
                EnvironmentShellStatus::Synchronizing,
                0,
                true,
                0,
            ),
        ],
        10,
    );
    assert!(local.changed);
    assert_eq!(coord.shown(&a), Some(&"cached-a-links"));

    // A becomes live while B still owes rows: A is held until t = 500 + 2000 = 2500.
    let waiting = [
        shell(
            a.clone(),
            Some("live-a"),
            Some(3),
            EnvironmentShellStatus::Live,
            0,
            true,
            0,
        ),
        shell(
            b.clone(),
            Some("cached-b"),
            Some(2),
            EnvironmentShellStatus::Synchronizing,
            0,
            true,
            0,
        ),
    ];
    let held = coord.step(&waiting, 500);
    assert_eq!(coord.shown(&a), Some(&"cached-a-links"));
    assert_eq!(held.release_at_ms, Some(2_500));
    assert!(coord.is_holding(&waiting));

    // Unchanged before deadline.
    let early = coord.step(&waiting, 2_499);
    assert!(!early.changed);
    assert_eq!(coord.shown(&a), Some(&"cached-a-links"));

    // Timer firing at deadline releases A.
    let token = held.timer_token.expect("timer token");
    let released = coord
        .on_release_timer(token, &waiting, 2_500)
        .expect("matching timer token fires");
    assert!(released.changed);
    assert_eq!(released.release_at_ms, None);
    assert_eq!(coord.shown(&a), Some(&"live-a"));
    assert!(!coord.is_holding(&waiting));
}

#[test]
fn steady_shells_releases_immediately_on_user_action() {
    let a = env("a");
    let b = env("b");
    let mut coord = SteadyShellCoordinator::new(STEADY_SHELL_RELEASE_WINDOW_MS);
    coord.mount();
    let _ = coord.step(
        &[
            shell(
                a.clone(),
                Some("live-a"),
                Some(3),
                EnvironmentShellStatus::Synchronizing,
                0,
                true,
                0,
            ),
            shell(
                b.clone(),
                Some("cached-b"),
                Some(2),
                EnvironmentShellStatus::Synchronizing,
                0,
                true,
                0,
            ),
        ],
        2_600,
    );
    let _ = coord.step(
        &[
            shell(
                a.clone(),
                Some("replayed-a"),
                Some(4),
                EnvironmentShellStatus::Synchronizing,
                0,
                true,
                0,
            ),
            shell(
                b.clone(),
                Some("cached-b"),
                Some(2),
                EnvironmentShellStatus::Synchronizing,
                0,
                true,
                0,
            ),
        ],
        2_610,
    );
    assert_eq!(coord.shown(&a), Some(&"live-a"));
    let _ = coord.step(
        &[
            shell(
                a.clone(),
                Some("replayed-a"),
                Some(4),
                EnvironmentShellStatus::Synchronizing,
                0,
                true,
                1,
            ),
            shell(
                b.clone(),
                Some("cached-b"),
                Some(2),
                EnvironmentShellStatus::Synchronizing,
                0,
                true,
                0,
            ),
        ],
        2_620,
    );
    assert_eq!(coord.shown(&a), Some(&"replayed-a"));
}

#[test]
fn steady_shells_fixes_bug_21_unmount_remount_and_multi_instance_isolation() {
    let a = env("a");
    let b = env("b");
    let mut coord_1 = SteadyShellCoordinator::new(STEADY_SHELL_RELEASE_WINDOW_MS);
    let mut coord_2: SteadyShellCoordinator<&'static str> =
        SteadyShellCoordinator::new(STEADY_SHELL_RELEASE_WINDOW_MS);
    coord_1.mount();
    coord_2.mount();

    let _ = coord_1.step(
        &[
            shell(
                a.clone(),
                Some("cached-a"),
                Some(1),
                EnvironmentShellStatus::Synchronizing,
                0,
                true,
                0,
            ),
            shell(
                b.clone(),
                Some("cached-b"),
                Some(2),
                EnvironmentShellStatus::Synchronizing,
                0,
                true,
                0,
            ),
        ],
        0,
    );
    let held = coord_1.step(
        &[
            shell(
                a.clone(),
                Some("live-a"),
                Some(3),
                EnvironmentShellStatus::Live,
                0,
                true,
                0,
            ),
            shell(
                b.clone(),
                Some("cached-b"),
                Some(2),
                EnvironmentShellStatus::Synchronizing,
                0,
                true,
                0,
            ),
        ],
        500,
    );
    let stale_token = held.timer_token.expect("token");
    assert_eq!(coord_1.release_at_ms(), Some(2_500));
    assert_eq!(coord_2.release_at_ms(), None);
    assert_eq!(coord_2.shown(&a), None);

    // Unmounting clears releaseAt/held snapshots and invalidates the old timer token.
    coord_1.unmount();
    assert_eq!(coord_1.release_at_ms(), None);
    assert_eq!(coord_1.shown(&a), None);
    assert!(
        coord_1
            .on_release_timer(
                stale_token,
                &[shell(
                    a.clone(),
                    Some("live-a"),
                    Some(3),
                    EnvironmentShellStatus::Live,
                    0,
                    true,
                    0
                )],
                2_500,
            )
            .is_none()
    );

    // Remounting starts fresh without leaking the old release window.
    coord_1.mount();
    assert!(
        coord_1
            .on_release_timer(
                stale_token,
                &[shell(
                    a,
                    Some("live-a"),
                    Some(3),
                    EnvironmentShellStatus::Live,
                    0,
                    true,
                    0
                )],
                2_500,
            )
            .is_none()
    );
}

proptest! {
    #[test]
    fn prop_steady_shells_never_leaks_release_at_past_deadline(
        window_ms in 100_u64..5_000,
        now_ms in 0_u64..100_000,
        a_live in any::<bool>(),
        b_connecting in any::<bool>(),
    ) {
        let a = env("a");
        let b = env("b");
        let mut coord = SteadyShellCoordinator::new(window_ms);
        let _ = coord.step(
            &[
                shell(a.clone(), Some("init-a"), Some(1), EnvironmentShellStatus::Synchronizing, 0, true, 0),
                shell(b.clone(), Some("init-b"), Some(1), EnvironmentShellStatus::Synchronizing, 0, b_connecting, 0),
            ],
            now_ms,
        );
        let next_status = if a_live {
            EnvironmentShellStatus::Live
        } else {
            EnvironmentShellStatus::Synchronizing
        };
        let step = coord.step(
            &[
                shell(a.clone(), Some("next-a"), Some(2), next_status, 1, true, 0),
                shell(b.clone(), Some("init-b"), Some(1), EnvironmentShellStatus::Synchronizing, 0, b_connecting, 0),
            ],
            now_ms + 10,
        );
        if let Some(deadline) = step.release_at_ms {
            prop_assert!(b_connecting);
            prop_assert_eq!(deadline, now_ms + 10 + window_ms);
            let done = coord.step(
                &[
                    shell(a.clone(), Some("next-a"), Some(2), next_status, 1, true, 0),
                    shell(b, Some("init-b"), Some(1), EnvironmentShellStatus::Synchronizing, 0, b_connecting, 0),
                ],
                deadline,
            );
            prop_assert_eq!(done.release_at_ms, None);
            prop_assert_eq!(coord.shown(&a), Some(&"next-a"));
        }
    }
}
