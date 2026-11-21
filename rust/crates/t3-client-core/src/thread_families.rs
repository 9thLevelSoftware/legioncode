//! Generation-keyed, ref-counted on-demand thread family cache and shell row merger.
//!
//! Mirrors `packages/client-runtime/src/state/threadFamilies.ts`:
//! - Keys family queries by `(EnvironmentId, ThreadId, generation)` where `generation == 0`
//!   waits for the session's first authoritative snapshot (`AwaitingAuthoritativeShell`).
//! - Never reuses a family read from an older generation once `generation` advances.
//! - Ref-counts active mounts (`mount_family`, `mount_rows`, idempotent `release`) and retains
//!   unmounted entries for an idle TTL of `60_000` ms (`THREAD_FAMILY_IDLE_TTL_MS`).
//! - Merges family rows behind live shell rows (`thread_family_rows_missing_from_shell`),
//!   respecting `olderSettled.asOfSequence` cuts and post-read `removedAt` tombstones.
//!
//! # Examples
//!
//! ```
//! use std::collections::BTreeMap;
//! use t3_client_core::thread_families::{
//!     ThreadFamilyCache, ThreadFamilyRead, ThreadFamilyRow, ThreadFamilySnapshot,
//! };
//! use t3_contracts::ids::{EnvironmentId, ThreadId};
//!
//! let env = EnvironmentId::new("env-1").expect("valid id");
//! let parent = ThreadId::new("parent").expect("valid id");
//! let child = ThreadId::new("idle-child").expect("valid id");
//!
//! let mut cache: ThreadFamilyCache<ThreadFamilyRow<&str>> = ThreadFamilyCache::default();
//! let mut handle = cache.mount_family(&env, &parent, 0);
//! assert_eq!(cache.read_state(&env, &parent), ThreadFamilyRead::Pending);
//! assert_eq!(cache.begin_fetch(&env, &parent, 0), None); // generation 0 waits
//!
//! cache.set_shell_generation(&env, 1, 10);
//! assert_eq!(cache.begin_fetch(&env, &parent, 10), Some(1));
//! cache.complete_fetch(
//!     &env,
//!     &parent,
//!     1,
//!     Ok(ThreadFamilySnapshot {
//!         snapshot_sequence: 3,
//!         threads: vec![ThreadFamilyRow::new(child.clone(), "idle")],
//!     }),
//!     true,
//!     20,
//! );
//! assert_eq!(cache.read_state(&env, &parent), ThreadFamilyRead::Settled);
//!
//! let merged = cache.merged_threads(
//!     &env,
//!     &[ThreadFamilyRow::new(parent.clone(), "live")],
//!     None,
//!     &BTreeMap::new(),
//! );
//! assert_eq!(merged.len(), 2);
//! assert!(cache.release(&mut handle, 30));
//! ```

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use t3_contracts::ids::{EnvironmentId, ThreadId};
use t3_contracts::orchestration::AppThread;

pub use crate::thread_families_cache::ThreadFamilyCache;

/// Idle retention duration in milliseconds for unmounted family queries (`60_000` ms).
pub const THREAD_FAMILY_IDLE_TTL_MS: u64 = 60_000;

/// Settledness state of an on-demand thread family read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThreadFamilyRead {
    /// Either awaiting authoritative shell (`generation == 0`) or fetch is in flight.
    Pending,
    /// Latest fetch for the active generation failed without an in-flight retry.
    Failed,
    /// Latest fetch for the active generation succeeded.
    Settled,
}

/// Trait implemented by thread shell rows merged by [`ThreadFamilyCache`].
pub trait HasThreadId {
    /// Returns this row's canonical [`ThreadId`].
    fn thread_id(&self) -> &ThreadId;
}

impl HasThreadId for ThreadId {
    fn thread_id(&self) -> &ThreadId {
        self
    }
}

impl HasThreadId for AppThread {
    fn thread_id(&self) -> &ThreadId {
        &self.id
    }
}

/// Lightweight wrapper pairing a [`ThreadId`] with arbitrary row payload `T`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadFamilyRow<T> {
    /// Canonical thread identifier.
    pub id: ThreadId,
    /// Associated row payload.
    pub payload: T,
}

impl<T> ThreadFamilyRow<T> {
    /// Creates a new [`ThreadFamilyRow`].
    pub const fn new(id: ThreadId, payload: T) -> Self {
        Self { id, payload }
    }
}

impl<T> HasThreadId for ThreadFamilyRow<T> {
    fn thread_id(&self) -> &ThreadId {
        &self.id
    }
}

/// Snapshot returned by `getThreadFamily` or mounted shelf/search reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadFamilySnapshot<T> {
    /// Server sequence at which these family rows were observed.
    pub snapshot_sequence: u64,
    /// Thread rows returned in this family or page.
    pub threads: Vec<T>,
}

impl<T> Default for ThreadFamilySnapshot<T> {
    fn default() -> Self {
        Self {
            snapshot_sequence: 0,
            threads: Vec::new(),
        }
    }
}

/// Returns family rows absent from `shell_threads`, dropping copies superseded by `removed_at`.
///
/// Shell rows always take precedence over family rows, and earlier family entries win over
/// duplicates in later families.
pub fn thread_family_rows_missing_from_shell<T: Clone + HasThreadId>(
    shell_threads: &[T],
    families: &[ThreadFamilySnapshot<T>],
    removed_at: &BTreeMap<ThreadId, u64>,
) -> Vec<T> {
    let mut present: BTreeSet<ThreadId> = shell_threads
        .iter()
        .map(|row| row.thread_id().clone())
        .collect();
    let mut missing = Vec::new();
    for family in families {
        for thread in &family.threads {
            let id = thread.thread_id();
            if present.contains(id) {
                continue;
            }
            if removed_at
                .get(id)
                .is_some_and(|&removed_seq| removed_seq > family.snapshot_sequence)
            {
                continue;
            }
            present.insert(id.clone());
            missing.push(thread.clone());
        }
    }
    missing
}

/// Idempotent release token returned by [`ThreadFamilyCache::mount_family`] / [`ThreadFamilyCache::mount_rows`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountHandle {
    pub(crate) environment_id: EnvironmentId,
    pub(crate) mount_key: String,
    pub(crate) released: bool,
}

impl MountHandle {
    /// Returns `true` if [`ThreadFamilyCache::release`] has already consumed this handle.
    pub const fn is_released(&self) -> bool {
        self.released
    }
}
