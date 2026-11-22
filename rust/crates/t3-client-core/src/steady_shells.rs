//! Synchronized multi-environment shell snapshot release coordination.
//!
//! Mirrors `packages/client-runtime/src/state/steadyShells.ts` while fixing BUG-21:
//! - In TypeScript, `createSteadyShellAtoms` mutated a closed-over `let state` inside a
//!   non-`keepAlive` `Atom.make` getter, leaking `releaseAt` and held snapshots across
//!   unmount/remount cycles and separate registries.
//! - [`SteadyShellCoordinator`] owns explicit `mount`, `unmount`, and generation-scoped
//!   `timer_token` validation so unmounted or stale release timers cannot leak state.
//!
//! # Examples
//!
//! ```
//! use t3_client_core::steady_shells::{
//!     EnvironmentShellStatus, STEADY_SHELL_RELEASE_WINDOW_MS, SteadyShellCoordinator,
//!     SteadyShellInput,
//! };
//! use t3_contracts::ids::EnvironmentId;
//!
//! let env_a = EnvironmentId::new("env-a").expect("valid id");
//! let env_b = EnvironmentId::new("env-b").expect("valid id");
//! let mut coordinator = SteadyShellCoordinator::new(STEADY_SHELL_RELEASE_WINDOW_MS);
//! coordinator.mount();
//!
//! let _ = coordinator.step(
//!     &[
//!         SteadyShellInput::new(env_a.clone(), Some("cached-a"), Some(1), EnvironmentShellStatus::Synchronizing, 0, true, 0),
//!         SteadyShellInput::new(env_b.clone(), Some("cached-b"), Some(2), EnvironmentShellStatus::Synchronizing, 0, true, 0),
//!     ],
//!     0,
//! );
//! let step = coordinator.step(
//!     &[
//!         SteadyShellInput::new(env_a.clone(), Some("live-a"), Some(3), EnvironmentShellStatus::Live, 0, true, 0),
//!         SteadyShellInput::new(env_b.clone(), Some("cached-b"), Some(2), EnvironmentShellStatus::Synchronizing, 0, true, 0),
//!     ],
//!     500,
//! );
//! assert_eq!(coordinator.shown(&env_a), Some(&"cached-a"));
//! assert_eq!(step.release_at_ms, Some(500 + STEADY_SHELL_RELEASE_WINDOW_MS));
//!
//! let released = coordinator.step(
//!     &[
//!         SteadyShellInput::new(env_a.clone(), Some("live-a"), Some(3), EnvironmentShellStatus::Live, 0, true, 0),
//!         SteadyShellInput::new(env_b.clone(), Some("live-b"), Some(4), EnvironmentShellStatus::Live, 0, true, 0),
//!     ],
//!     900,
//! );
//! assert_eq!(coordinator.shown(&env_a), Some(&"live-a"));
//! assert_eq!(coordinator.shown(&env_b), Some(&"live-b"));
//! assert_eq!(released.release_at_ms, None);
//! ```

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use t3_contracts::ids::EnvironmentId;

pub use crate::steady_shells_resume::{SteadyShellCoordinator, SteadyShellStep};

/// Default cross-environment release window in milliseconds (`2_000` ms).
pub const STEADY_SHELL_RELEASE_WINDOW_MS: u64 = 2_000;

/// Synchronization lifecycle status of an environment's shell state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EnvironmentShellStatus {
    /// No snapshot has been loaded yet.
    Empty,
    /// Showing persisted cached rows while disconnected.
    Cached,
    /// Connected or connecting and replaying shell stream items before `synchronized`.
    Synchronizing,
    /// Received `synchronized` and following live deltas.
    Live,
}

/// Per-environment input fed to [`step_steady_shells`] and [`SteadyShellCoordinator::step`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteadyShellInput<S> {
    /// Target environment identifier.
    pub environment_id: EnvironmentId,
    /// Current live snapshot held for this environment, if any.
    pub snapshot: Option<S>,
    /// Authoritative sequence carried on `snapshot` (`snapshotSequence`).
    pub server_sequence: Option<u64>,
    /// Current shell synchronization status.
    pub status: EnvironmentShellStatus,
    /// Monotonic shell generation incremented on each authoritative server snapshot.
    pub generation: u64,
    /// Whether the connection is currently connecting or connected (so peers may wait on it).
    pub connecting: bool,
    /// Monotonic counter incremented when the user moves or removes a row in this environment.
    pub row_actions: u64,
}

impl<S> SteadyShellInput<S> {
    /// Creates a [`SteadyShellInput`] from explicit components.
    pub const fn new(
        environment_id: EnvironmentId,
        snapshot: Option<S>,
        server_sequence: Option<u64>,
        status: EnvironmentShellStatus,
        generation: u64,
        connecting: bool,
        row_actions: u64,
    ) -> Self {
        Self {
            environment_id,
            snapshot,
            server_sequence,
            status,
            generation,
            connecting,
            row_actions,
        }
    }
}
/// Displayed shell record stored per environment inside [`SteadyShellsState`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteadyShell<S> {
    /// Currently displayed snapshot for thread lists.
    pub shown: Option<S>,
    /// Server sequence of `shown`.
    pub shown_sequence: Option<u64>,
    /// Generation of `shown`.
    pub shown_generation: u64,
    /// True when rows were last shown while `Live`, so live updates display immediately.
    pub following: bool,
    /// True when the displayed rows already reflect a server snapshot of the synchronizing link.
    pub caught_up: bool,
    /// `row_actions` watermark when rows last displayed while `Live` or initially.
    pub row_actions: u64,
}
/// Pure snapshot state returned by [`step_steady_shells`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteadyShellsState<S> {
    /// Displayed shells keyed by environment ID.
    pub shells: BTreeMap<EnvironmentId, SteadyShell<S>>,
    /// Latest wall-clock deadline (`now_ms`) when held rows must release; `None` if nothing waits.
    pub release_at_ms: Option<u64>,
}

impl<S> Default for SteadyShellsState<S> {
    fn default() -> Self {
        Self {
            shells: BTreeMap::new(),
            release_at_ms: None,
        }
    }
}

fn build_shown_shell<S: Clone>(
    input: &SteadyShellInput<S>,
    caught_up: bool,
    current: Option<&SteadyShell<S>>,
) -> SteadyShell<S> {
    let row_actions = match current {
        Some(prev) if input.status != EnvironmentShellStatus::Live => prev.row_actions,
        _ => input.row_actions,
    };
    SteadyShell {
        shown: input.snapshot.clone(),
        shown_sequence: input.server_sequence,
        shown_generation: input.generation,
        following: input.status == EnvironmentShellStatus::Live,
        caught_up,
        row_actions,
    }
}

fn keep_shell<S>(
    shells: &mut BTreeMap<EnvironmentId, SteadyShell<S>>,
    others_owe_rows: &mut bool,
    input: &SteadyShellInput<S>,
    next: SteadyShell<S>,
) {
    if input.status == EnvironmentShellStatus::Synchronizing && input.connecting && !next.caught_up
    {
        *others_owe_rows = true;
    }
    shells.insert(input.environment_id.clone(), next);
}

/// Advances [`SteadyShellsState`] for one batch of `inputs` at `now_ms`.
///
/// Returns `None` when the resulting shells and `release_at_ms` are identical to `previous`,
/// allowing callers to avoid redundant UI notifications.
pub fn step_steady_shells<S: Clone + PartialEq>(
    previous: &SteadyShellsState<S>,
    inputs: &[SteadyShellInput<S>],
    now_ms: u64,
    window_ms: u64,
) -> Option<SteadyShellsState<S>> {
    let mut shells = BTreeMap::new();
    let mut waiting: Vec<&SteadyShellInput<S>> = Vec::new();
    let mut others_owe_rows = false;
    for input in inputs {
        let synchronizing = input.status == EnvironmentShellStatus::Synchronizing;
        let Some(current) = previous.shells.get(&input.environment_id) else {
            keep_shell(
                &mut shells,
                &mut others_owe_rows,
                input,
                build_shown_shell(input, false, None),
            );
            continue;
        };
        let new_snapshot = input.generation > current.shown_generation;
        let local = current.shown.is_none()
            || (input.server_sequence == current.shown_sequence && !new_snapshot);
        let user_acted = input.row_actions != current.row_actions;
        if local
            || user_acted
            || (input.status == EnvironmentShellStatus::Live && current.following)
        {
            let caught_up = synchronizing && (current.caught_up || new_snapshot);
            let shown = build_shown_shell(input, caught_up, Some(current));
            keep_shell(&mut shells, &mut others_owe_rows, input, shown);
        } else if !synchronizing || new_snapshot {
            waiting.push(input);
            shells.insert(input.environment_id.clone(), current.clone());
        } else {
            let mut held = current.clone();
            held.following = false;
            keep_shell(&mut shells, &mut others_owe_rows, input, held);
        }
    }
    let mut release_at_ms = None;
    if !waiting.is_empty() {
        let deadline = previous
            .release_at_ms
            .unwrap_or_else(|| now_ms.saturating_add(window_ms));
        if !others_owe_rows || now_ms >= deadline {
            for input in waiting {
                let current = shells.get(&input.environment_id).cloned();
                let next = build_shown_shell(
                    input,
                    input.status == EnvironmentShellStatus::Synchronizing,
                    current.as_ref(),
                );
                shells.insert(input.environment_id.clone(), next);
            }
        } else {
            release_at_ms = Some(deadline);
        }
    }
    if release_at_ms == previous.release_at_ms && shells == previous.shells {
        None
    } else {
        Some(SteadyShellsState {
            shells,
            release_at_ms,
        })
    }
}
