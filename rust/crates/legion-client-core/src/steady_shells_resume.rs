//! Steady-shell lifecycle coordinator (split from [`crate::steady_shells`]) and automatic
//! full-snapshot thread stream resume recovery (`73e6472545` / #24583).

use std::collections::BTreeMap;

use legion_contracts::ids::EnvironmentId;
use serde::{Deserialize, Serialize};

use crate::steady_shells::{
    STEADY_SHELL_RELEASE_WINDOW_MS, SteadyShellInput, SteadyShellsState, step_steady_shells,
};

/// Result emitted by [`SteadyShellCoordinator::step`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteadyShellStep {
    /// Whether any displayed snapshot or release deadline changed.
    pub changed: bool,
    /// Pending release deadline in milliseconds if rows are currently held.
    pub release_at_ms: Option<u64>,
    /// Generation-scoped timer token to pass back to [`SteadyShellCoordinator::on_release_timer`].
    pub timer_token: Option<u64>,
}

/// Owned, mount-scoped steady-shell coordinator fixing BUG-21 closure-state leakage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteadyShellCoordinator<S> {
    window_ms: u64,
    mounted: bool,
    timer_generation: u64,
    state: SteadyShellsState<S>,
}

impl<S> Default for SteadyShellCoordinator<S> {
    fn default() -> Self {
        Self::new(STEADY_SHELL_RELEASE_WINDOW_MS)
    }
}

impl<S> SteadyShellCoordinator<S> {
    /// Creates an unmounted [`SteadyShellCoordinator`] with release window `window_ms`.
    pub const fn new(window_ms: u64) -> Self {
        Self {
            window_ms,
            mounted: false,
            timer_generation: 0,
            state: SteadyShellsState {
                shells: BTreeMap::new(),
                release_at_ms: None,
            },
        }
    }

    /// Returns whether this coordinator instance is currently mounted.
    pub const fn is_mounted(&self) -> bool {
        self.mounted
    }

    /// Returns the currently held release deadline, if any.
    pub const fn release_at_ms(&self) -> Option<u64> {
        self.state.release_at_ms
    }

    /// Returns the snapshot currently shown for `environment_id`.
    pub fn shown(&self, environment_id: &EnvironmentId) -> Option<&S> {
        self.state
            .shells
            .get(environment_id)
            .and_then(|shell| shell.shown.as_ref())
    }

    /// Returns the full internal [`SteadyShellsState`].
    pub const fn state(&self) -> &SteadyShellsState<S> {
        &self.state
    }

    /// Mounts a fresh lifecycle epoch, clearing any stale held state.
    pub fn mount(&mut self) {
        self.mounted = true;
        self.state = SteadyShellsState::default();
        self.timer_generation = self.timer_generation.saturating_add(1);
    }

    /// Unmounts the coordinator, clearing held snapshots and invalidating pending timer tokens.
    pub fn unmount(&mut self) {
        self.mounted = false;
        self.state = SteadyShellsState::default();
        self.timer_generation = self.timer_generation.saturating_add(1);
    }
}

impl<S: Clone + PartialEq> SteadyShellCoordinator<S> {
    /// Steps the steady-shell state machine for `inputs` at `now_ms`.
    ///
    /// Implicitly marks the coordinator mounted if called before [`Self::mount`].
    pub fn step(&mut self, inputs: &[SteadyShellInput<S>], now_ms: u64) -> SteadyShellStep {
        if !self.mounted {
            self.mounted = true;
        }
        let prev_release = self.state.release_at_ms;
        let next = step_steady_shells(&self.state, inputs, now_ms, self.window_ms);
        let changed = if let Some(updated) = next {
            self.state = updated;
            true
        } else {
            false
        };
        if self.state.release_at_ms != prev_release {
            self.timer_generation = self.timer_generation.saturating_add(1);
        }
        SteadyShellStep {
            changed,
            release_at_ms: self.state.release_at_ms,
            timer_token: self.state.release_at_ms.map(|_| self.timer_generation),
        }
    }

    /// Fires a previously scheduled release timer only if `token` matches and still mounted.
    pub fn on_release_timer(
        &mut self,
        token: u64,
        inputs: &[SteadyShellInput<S>],
        now_ms: u64,
    ) -> Option<SteadyShellStep> {
        if !self.mounted || token != self.timer_generation || self.state.release_at_ms.is_none() {
            return None;
        }
        Some(self.step(inputs, now_ms))
    }

    /// Returns `true` when any input holds a newer live snapshot than currently shown.
    pub fn is_holding(&self, inputs: &[SteadyShellInput<S>]) -> bool {
        inputs.iter().any(|input| {
            let shown = self
                .state
                .shells
                .get(&input.environment_id)
                .and_then(|s| s.shown.as_ref());
            shown != input.snapshot.as_ref()
        })
    }
}

/// Thread detail hydration state driving UI sync indicator (`73e6472545` / #24583).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadDetailDataStatus {
    /// No local cache and no live stream data yet.
    Empty,
    /// Showing cached projection while synchronizing live stream.
    Cached,
    /// Live projection synchronized cleanly.
    Live,
}

/// Resolved sync status banner shown over cached or loading messages (`#24583`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadSyncPhase {
    /// First-time load with no cached messages yet.
    Loading,
    /// Cached messages shown while stream synchronizes without error.
    Syncing,
    /// Cached or empty detail hit a stream/sync failure (avoids spinning forever).
    Failed,
    /// Stream synchronized and live.
    Live,
}

/// Resolves the UI thread sync indicator phase (`#24583`).
#[must_use]
pub const fn resolve_thread_sync_phase(
    status: ThreadDetailDataStatus,
    has_error: bool,
) -> ThreadSyncPhase {
    if has_error {
        return ThreadSyncPhase::Failed;
    }
    match status {
        ThreadDetailDataStatus::Empty => ThreadSyncPhase::Loading,
        ThreadDetailDataStatus::Cached => ThreadSyncPhase::Syncing,
        ThreadDetailDataStatus::Live => ThreadSyncPhase::Live,
    }
}

/// Recovery action emitted when a thread stream exits or fails (`#24583`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadResumeRecovery {
    /// Failed during incremental replay from cached cursor -> drop cursor and immediately reload
    /// full snapshot without surfacing an error.
    ImmediateFullSnapshotReload,
    /// Failed during full snapshot load -> record error and wait for backoff before retrying full snapshot.
    BackoffFullSnapshotReload,
}

/// Pure resume-recovery state machine for a subscribed thread detail stream (`#24583`).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ThreadStreamResumeState {
    cached_sequence: Option<i64>,
    resuming: bool,
    force_snapshot: bool,
    error: Option<String>,
}

impl ThreadStreamResumeState {
    /// Creates a new stream state with an optional cached snapshot sequence cursor.
    #[must_use]
    pub const fn new(cached_sequence: Option<i64>) -> Self {
        Self {
            cached_sequence,
            resuming: false,
            force_snapshot: false,
            error: None,
        }
    }

    /// Returns the stored snapshot sequence cursor, if any.
    #[must_use]
    pub const fn cached_sequence(&self) -> Option<i64> {
        self.cached_sequence
    }

    /// Returns whether `force_snapshot` is currently armed due to an earlier replay failure.
    #[must_use]
    pub const fn force_snapshot(&self) -> bool {
        self.force_snapshot
    }

    /// Returns the surfaced sync error, if any (`None` when silently recovering via snapshot).
    #[must_use]
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Computes `afterSequence` for the next subscription attempt (`None` triggers a full snapshot).
    #[must_use]
    pub fn begin_subscription(&mut self) -> Option<i64> {
        let can_resume = self.cached_sequence.is_some() && !self.force_snapshot;
        self.resuming = can_resume;
        if can_resume {
            self.cached_sequence
        } else {
            None
        }
    }

    /// Records receipt of a full thread snapshot, clearing `force_snapshot` and resuming future
    /// reconnects from `snapshot_sequence`.
    pub fn on_snapshot_received(&mut self, snapshot_sequence: i64) {
        self.force_snapshot = false;
        self.resuming = false;
        self.error = None;
        self.cached_sequence = Some(snapshot_sequence);
    }

    /// Advances the tracked cursor after applying an incremental delta safely.
    pub fn on_incremental_sequence(&mut self, sequence: i64) {
        self.cached_sequence = Some(
            self.cached_sequence
                .map_or(sequence, |cur| cur.max(sequence)),
        );
    }

    /// Handles a replay/decode defect or stream interruption (`#24583`).
    pub fn on_stream_defect(&mut self, reason: impl Into<String>) -> ThreadResumeRecovery {
        self.force_snapshot = true;
        if self.resuming {
            self.resuming = false;
            self.error = None;
            ThreadResumeRecovery::ImmediateFullSnapshotReload
        } else {
            self.error = Some(reason.into());
            ThreadResumeRecovery::BackoffFullSnapshotReload
        }
    }

    /// Arms full snapshot reload on session replacement after repeated failures.
    pub fn arm_force_snapshot(&mut self) {
        self.force_snapshot = true;
    }
}
