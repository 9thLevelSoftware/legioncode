//! Cache storage and mount lifecycle for [`ThreadFamilyCache`].

use std::collections::BTreeMap;

use legion_contracts::ids::{EnvironmentId, ThreadId};

use crate::thread_families::{
    HasThreadId, MountHandle, THREAD_FAMILY_IDLE_TTL_MS, ThreadFamilyRead, ThreadFamilySnapshot,
    thread_family_rows_missing_from_shell,
};

#[derive(Debug, Clone, PartialEq, Eq)]
enum FamilyEntryState<T> {
    Pending,
    Failed { error: String },
    Settled(ThreadFamilySnapshot<T>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FamilyCacheEntry<T> {
    state: FamilyEntryState<T>,
    idle_since_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum MountSource<T> {
    Family(ThreadId),
    Rows(Vec<ThreadFamilySnapshot<T>>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MountEntry<T> {
    ref_count: usize,
    source: MountSource<T>,
}

type FamilyCacheKey = (EnvironmentId, ThreadId, u64);

/// Generation-keyed, ref-counted cache of on-demand thread families and mounted row pages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadFamilyCache<T> {
    idle_ttl_ms: u64,
    generations: BTreeMap<EnvironmentId, u64>,
    entries: BTreeMap<FamilyCacheKey, FamilyCacheEntry<T>>,
    mounts: BTreeMap<EnvironmentId, BTreeMap<String, MountEntry<T>>>,
}

impl<T> Default for ThreadFamilyCache<T> {
    fn default() -> Self {
        Self::new(THREAD_FAMILY_IDLE_TTL_MS)
    }
}

impl<T> ThreadFamilyCache<T> {
    /// Creates an empty [`ThreadFamilyCache`] with idle eviction TTL `idle_ttl_ms`.
    pub const fn new(idle_ttl_ms: u64) -> Self {
        Self {
            idle_ttl_ms,
            generations: BTreeMap::new(),
            entries: BTreeMap::new(),
            mounts: BTreeMap::new(),
        }
    }

    /// Returns the active shell generation for `environment_id` (`0` before authoritative snapshot).
    pub fn shell_generation(&self, environment_id: &EnvironmentId) -> u64 {
        self.generations.get(environment_id).copied().unwrap_or(0)
    }

    /// Updates `environment_id`'s shell generation and marks superseded entries idle at `now_ms`.
    pub fn set_shell_generation(
        &mut self,
        environment_id: &EnvironmentId,
        generation: u64,
        now_ms: u64,
    ) {
        let prev = self.shell_generation(environment_id);
        if prev == generation {
            return;
        }
        self.generations.insert(environment_id.clone(), generation);
        for ((env_id, _, entry_gen), entry) in &mut self.entries {
            if env_id == environment_id && *entry_gen != generation && entry.idle_since_ms.is_none()
            {
                entry.idle_since_ms = Some(now_ms);
            }
        }
    }

    /// Returns the current [`ThreadFamilyRead`] status for `(environment_id, thread_id)`.
    pub fn read_state(
        &self,
        environment_id: &EnvironmentId,
        thread_id: &ThreadId,
    ) -> ThreadFamilyRead {
        let generation = self.shell_generation(environment_id);
        if generation == 0 {
            return ThreadFamilyRead::Pending;
        }
        let key = (environment_id.clone(), thread_id.clone(), generation);
        match self.entries.get(&key).map(|entry| &entry.state) {
            Some(FamilyEntryState::Failed { .. }) => ThreadFamilyRead::Failed,
            Some(FamilyEntryState::Settled(_)) => ThreadFamilyRead::Settled,
            Some(FamilyEntryState::Pending) | None => ThreadFamilyRead::Pending,
        }
    }

    /// Returns the current ref-count for `mount_key` in `environment_id`.
    pub fn mount_ref_count(&self, environment_id: &EnvironmentId, mount_key: &str) -> usize {
        self.mounts
            .get(environment_id)
            .and_then(|env_mounts| env_mounts.get(mount_key))
            .map_or(0, |entry| entry.ref_count)
    }

    /// Number of cached `(env, thread, generation)` family entries currently retained.
    pub fn cached_entry_count(&self) -> usize {
        self.entries.len()
    }

    /// Mounts a thread's family into merged snapshots until [`Self::release`] drops the ref count.
    ///
    /// If the current generation's cached entry previously failed, resets it to `Pending` so
    /// mounting retries rather than preserving a stale failure for the rest of the session.
    pub fn mount_family(
        &mut self,
        environment_id: &EnvironmentId,
        thread_id: &ThreadId,
        now_ms: u64,
    ) -> MountHandle {
        self.evict_expired(now_ms);
        let mount_key = family_mount_key(thread_id);
        let env_mounts = self.mounts.entry(environment_id.clone()).or_default();
        if let Some(entry) = env_mounts.get_mut(&mount_key) {
            entry.ref_count = entry.ref_count.saturating_add(1);
        } else {
            env_mounts.insert(
                mount_key.clone(),
                MountEntry {
                    ref_count: 1,
                    source: MountSource::Family(thread_id.clone()),
                },
            );
        }
        let generation = self.shell_generation(environment_id);
        if generation > 0 {
            let key = (environment_id.clone(), thread_id.clone(), generation);
            if let Some(cached) = self.entries.get_mut(&key) {
                cached.idle_since_ms = None;
                if matches!(cached.state, FamilyEntryState::Failed { .. }) {
                    cached.state = FamilyEntryState::Pending;
                }
            }
        }
        MountHandle {
            environment_id: environment_id.clone(),
            mount_key,
            released: false,
        }
    }

    /// Mounts external read `rows` (such as shelf pages or search results) under `mount_key`.
    pub fn mount_rows(
        &mut self,
        environment_id: &EnvironmentId,
        mount_key: impl Into<String>,
        rows: Vec<ThreadFamilySnapshot<T>>,
        now_ms: u64,
    ) -> MountHandle {
        self.evict_expired(now_ms);
        let key_str = mount_key.into();
        let env_mounts = self.mounts.entry(environment_id.clone()).or_default();
        if let Some(entry) = env_mounts.get_mut(&key_str) {
            entry.ref_count = entry.ref_count.saturating_add(1);
        } else {
            env_mounts.insert(
                key_str.clone(),
                MountEntry {
                    ref_count: 1,
                    source: MountSource::Rows(rows),
                },
            );
        }
        MountHandle {
            environment_id: environment_id.clone(),
            mount_key: key_str,
            released: false,
        }
    }

    /// Idempotently releases one mount reference, starting the 60s idle TTL when refs hit `0`.
    pub fn release(&mut self, handle: &mut MountHandle, now_ms: u64) -> bool {
        if handle.released {
            return false;
        }
        handle.released = true;
        let Some(env_mounts) = self.mounts.get_mut(&handle.environment_id) else {
            return true;
        };
        let Some(entry) = env_mounts.get_mut(&handle.mount_key) else {
            return true;
        };
        if entry.ref_count > 1 {
            entry.ref_count -= 1;
            return true;
        }
        let removed = env_mounts.remove(&handle.mount_key);
        if env_mounts.is_empty() {
            self.mounts.remove(&handle.environment_id);
        }
        if let Some(MountEntry {
            source: MountSource::Family(thread_id),
            ..
        }) = removed
        {
            let generation = self.shell_generation(&handle.environment_id);
            if generation > 0 {
                let key = (handle.environment_id.clone(), thread_id, generation);
                if let Some(cached) = self.entries.get_mut(&key) {
                    cached.idle_since_ms = Some(now_ms);
                }
            }
        }
        true
    }

    /// Marks the active generation's family fetch as in-flight, returning its generation if `!= 0`.
    pub fn begin_fetch(
        &mut self,
        environment_id: &EnvironmentId,
        thread_id: &ThreadId,
        now_ms: u64,
    ) -> Option<u64> {
        self.evict_expired(now_ms);
        let generation = self.shell_generation(environment_id);
        if generation == 0 {
            return None;
        }
        let key = (environment_id.clone(), thread_id.clone(), generation);
        let mounted = self.mount_ref_count(environment_id, &family_mount_key(thread_id)) > 0;
        let idle_since_ms = if mounted { None } else { Some(now_ms) };
        self.entries.insert(
            key,
            FamilyCacheEntry {
                state: FamilyEntryState::Pending,
                idle_since_ms,
            },
        );
        Some(generation)
    }

    /// Forces a refresh of the active generation's cached family entry (`generation != 0`).
    pub fn refresh(
        &mut self,
        environment_id: &EnvironmentId,
        thread_id: &ThreadId,
        now_ms: u64,
    ) -> Option<u64> {
        self.begin_fetch(environment_id, thread_id, now_ms)
    }

    /// Stores a completed fetch result under `(environment_id, thread_id, generation)`.
    ///
    /// When `shell_thread_families_enabled` is `false`, stores an empty settled family so
    /// absent threads resolve cleanly as gone instead of pending forever.
    pub fn complete_fetch(
        &mut self,
        environment_id: &EnvironmentId,
        thread_id: &ThreadId,
        generation: u64,
        result: Result<ThreadFamilySnapshot<T>, String>,
        shell_thread_families_enabled: bool,
        now_ms: u64,
    ) {
        if generation == 0 {
            return;
        }
        let key = (environment_id.clone(), thread_id.clone(), generation);
        let active_mount = generation == self.shell_generation(environment_id)
            && self.mount_ref_count(environment_id, &family_mount_key(thread_id)) > 0;
        let state = match result {
            Ok(_) if !shell_thread_families_enabled => {
                FamilyEntryState::Settled(ThreadFamilySnapshot::default())
            }
            Ok(snapshot) => FamilyEntryState::Settled(snapshot),
            Err(error) => FamilyEntryState::Failed { error },
        };
        self.entries.insert(
            key,
            FamilyCacheEntry {
                state,
                idle_since_ms: if active_mount { None } else { Some(now_ms) },
            },
        );
    }

    /// Evicts unmounted cache entries whose idle age has reached `idle_ttl_ms`.
    pub fn evict_expired(&mut self, now_ms: u64) -> usize {
        let before = self.entries.len();
        let ttl = self.idle_ttl_ms;
        self.entries.retain(|_, entry| {
            entry
                .idle_since_ms
                .is_none_or(|since| now_ms.saturating_sub(since) < ttl)
        });
        before - self.entries.len()
    }
}

impl<T: Clone + HasThreadId> ThreadFamilyCache<T> {
    /// Merges mounted family rows behind `shell_threads`, honoring cuts and `removed_at`.
    pub fn merged_threads(
        &self,
        environment_id: &EnvironmentId,
        shell_threads: &[T],
        older_settled_as_of_sequence: Option<u64>,
        removed_at: &BTreeMap<ThreadId, u64>,
    ) -> Vec<T> {
        let Some(env_mounts) = self.mounts.get(environment_id) else {
            return shell_threads.to_vec();
        };
        if env_mounts.is_empty() {
            return shell_threads.to_vec();
        }
        let cut = older_settled_as_of_sequence.unwrap_or(0);
        let generation = self.shell_generation(environment_id);
        let mut eligible_families = Vec::new();
        for mount in env_mounts.values() {
            match &mount.source {
                MountSource::Family(thread_id) => {
                    if generation == 0 {
                        continue;
                    }
                    let key = (environment_id.clone(), thread_id.clone(), generation);
                    if let Some(FamilyCacheEntry {
                        state: FamilyEntryState::Settled(snapshot),
                        ..
                    }) = self.entries.get(&key)
                        && snapshot.snapshot_sequence >= cut
                    {
                        eligible_families.push(snapshot.clone());
                    }
                }
                MountSource::Rows(rows) => {
                    for snapshot in rows {
                        if snapshot.snapshot_sequence >= cut {
                            eligible_families.push(snapshot.clone());
                        }
                    }
                }
            }
        }
        let extras =
            thread_family_rows_missing_from_shell(shell_threads, &eligible_families, removed_at);
        if extras.is_empty() {
            return shell_threads.to_vec();
        }
        let mut merged = Vec::with_capacity(shell_threads.len() + extras.len());
        merged.extend_from_slice(shell_threads);
        merged.extend(extras);
        merged
    }
}

fn family_mount_key(thread_id: &ThreadId) -> String {
    format!("family:{}", thread_id.as_str())
}
