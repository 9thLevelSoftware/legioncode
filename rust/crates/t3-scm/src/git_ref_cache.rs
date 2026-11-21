//! Ref/config fingerprint caches and invocation-budgeted git queries (`220a2b86e2`, `a0ca0bfaf2`,
//! `370b1fde77`, `2c01259fb4`, `c1464d6774`, `d57b83d85e`, `38d27db746`, `2af435185e`).

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use sha2::{Digest, Sha256};

pub use crate::git_poll_queries::{
    BranchListSnapshot, BranchPrGitContext, RemotePollMetadata, read_remote_poll_metadata,
    reload_branch_list_budgeted, resolve_branch_pr_git_context,
};

/// Default TTL (`5m`) for cached base branch resolution and remote presence (`a0ca0bfaf2`).
pub const REF_CACHE_TTL_MS: u64 = 300_000;
/// Maximum cached resolved-base entries (`a0ca0bfaf2`).
pub const BASE_BRANCH_CACHE_CAPACITY: usize = 2_048;
/// Maximum git subprocesses permitted for a full branch-list reload (`d57b83d85e`).
pub const BRANCH_LIST_MAX_GIT_INVOCATIONS: usize = 6;
/// Maximum git subprocesses permitted for a single branch PR lookup (`2af435185e`).
pub const BRANCH_PR_LOOKUP_MAX_GIT_INVOCATIONS: usize = 9;

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

fn file_stat_token(path: &Path) -> String {
    let Ok(meta) = std::fs::metadata(path) else {
        return "-".to_owned();
    };
    let len = meta.len();
    let mtime_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis());
    format!("{len}:{mtime_ms}")
}

/// Computes a deterministic fingerprint over `HEAD`, `packed-refs`, `reftable/tables.list`, and sorted
/// loose ref files under `refs/{heads,remotes}` while deliberately **ignoring directory mtimes**
/// (so Git for Windows' no-op `fetch --dry-run` cannot bust caches, `370b1fde77`).
#[must_use]
pub fn compute_base_branch_inputs_stamp(
    git_common_dir: &Path,
    effective_config_list_z: &[u8],
) -> String {
    let mut parts: Vec<String> = vec![
        file_stat_token(&git_common_dir.join("packed-refs")),
        file_stat_token(&git_common_dir.join("reftable").join("tables.list")),
    ];

    let loose_refs = collect_sorted_loose_ref_paths(git_common_dir);
    for ref_path in loose_refs {
        let rel = ref_path
            .strip_prefix(git_common_dir)
            .unwrap_or(&ref_path)
            .to_string_lossy()
            .replace('\\', "/");
        parts.push(format!("{rel}={}", file_stat_token(&ref_path)));
    }
    parts.push(sha256_hex(effective_config_list_z));
    parts.join("\0")
}

/// Reads a lightweight ref snapshot hash (`HEAD`, `packed-refs`, loose refs) used to skip idle local
/// status re-reads when refs did not move (`220a2b86e2`).
#[must_use]
pub fn read_ref_snapshot(git_common_dir: &Path) -> Option<String> {
    if !git_common_dir.is_dir() {
        return None;
    }
    let mut material = file_stat_token(&git_common_dir.join("HEAD"));
    if let Ok(head) = std::fs::read_to_string(git_common_dir.join("HEAD")) {
        material.push('\0');
        material.push_str(head.trim());
    }
    material.push('\0');
    material.push_str(&compute_base_branch_inputs_stamp(git_common_dir, &[]));
    Some(sha256_hex(material.as_bytes()))
}

fn collect_sorted_loose_ref_paths(git_common_dir: &Path) -> Vec<PathBuf> {
    let mut queue = VecDeque::from([
        git_common_dir.join("refs").join("heads"),
        git_common_dir.join("refs").join("remotes"),
    ]);
    let mut loose = Vec::new();
    while let Some(dir) = queue.pop_front() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut child_dirs = Vec::new();
        let mut child_files = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                child_dirs.push(path);
            } else {
                child_files.push(path);
            }
        }
        child_dirs.sort();
        child_files.sort();
        queue.extend(child_dirs);
        loose.extend(child_files);
    }
    loose
}

/// Tracks the last seen ref snapshot per repository cwd so idle status polls skip redundant local
/// `git status` reads when refs have not moved (`220a2b86e2`).
#[derive(Debug, Default)]
pub struct IdleLocalStatusTracker {
    refs_at_local_read: HashMap<String, String>,
}

impl IdleLocalStatusTracker {
    /// Creates an empty [`IdleLocalStatusTracker`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `true` when `cwd` needs a local status read (`None` snapshot or refs moved since last read).
    pub fn should_refresh_local(&mut self, cwd: &str, current_snapshot: Option<&str>) -> bool {
        let Some(snap) = current_snapshot else {
            self.refs_at_local_read.remove(cwd);
            return true;
        };
        if self.refs_at_local_read.get(cwd).map(String::as_str) == Some(snap) {
            return false;
        }
        self.refs_at_local_read
            .insert(cwd.to_owned(), snap.to_owned());
        true
    }

    /// Drops the remembered snapshot for `cwd` so the next poll forces a local refresh.
    pub fn invalidate(&mut self, cwd: &str) {
        self.refs_at_local_read.remove(cwd);
    }
}

/// Cache key for resolved no-upstream base branches (`a0ca0bfaf2`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BaseBranchCacheKey {
    /// Canonical repository `cwd`.
    pub cwd: String,
    /// Current local branch name.
    pub branch: String,
    /// Whether `origin/<branch>` is eligible.
    pub allow_remote_of_current: bool,
    /// Combined ref/config input stamp from [`compute_base_branch_inputs_stamp`].
    pub inputs_stamp: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CachedBaseEntry {
    expires_at_ms: u64,
    base_branch: Option<String>,
}

/// Bounded fingerprint cache of resolved base branches (`a0ca0bfaf2`).
#[derive(Debug, Default)]
pub struct BaseBranchCache {
    order: VecDeque<BaseBranchCacheKey>,
    entries: HashMap<BaseBranchCacheKey, CachedBaseEntry>,
}

impl BaseBranchCache {
    /// Creates an empty [`BaseBranchCache`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Looks up a valid cached base branch while `inputs_stamp` matches and TTL has not expired.
    pub fn get(&mut self, key: &BaseBranchCacheKey, now_ms: u64) -> Option<Option<&str>> {
        let expired = self
            .entries
            .get(key)
            .is_some_and(|entry| now_ms > entry.expires_at_ms);
        if expired {
            self.entries.remove(key);
            self.order.retain(|k| k != key);
            return None;
        }
        self.entries.get(key).map(|e| e.base_branch.as_deref())
    }

    /// Stores a resolved base branch under `key`.
    pub fn insert(&mut self, key: BaseBranchCacheKey, base_branch: Option<String>, now_ms: u64) {
        if self.entries.contains_key(&key) {
            self.order.retain(|k| k != &key);
        }
        self.order.push_back(key.clone());
        self.entries.insert(
            key,
            CachedBaseEntry {
                expires_at_ms: now_ms.saturating_add(REF_CACHE_TTL_MS),
                base_branch,
            },
        );
        while self.entries.len() > BASE_BRANCH_CACHE_CAPACITY {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            } else {
                break;
            }
        }
    }
}

/// Caches positive remote existence (`true` for `5m`) while never caching `false` (`c1464d6774`)
/// so remotes added outside T3 Code appear immediately while local-only repos skip remote lookups.
#[derive(Debug, Default)]
pub struct PrimaryRemoteExistsCache {
    positive_until_ms: HashMap<String, u64>,
}

impl PrimaryRemoteExistsCache {
    /// Creates an empty [`PrimaryRemoteExistsCache`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `Some(true)` on a live positive hit, or `None` when unknown or expired.
    #[must_use]
    pub fn get(&self, git_common_dir: &str, now_ms: u64) -> Option<bool> {
        self.positive_until_ms
            .get(git_common_dir)
            .filter(|&&exp| now_ms <= exp)
            .map(|_| true)
    }

    /// Records whether `git_common_dir` currently has at least one remote (`false` evicts immediately).
    pub fn record(&mut self, git_common_dir: &str, has_remote: bool, now_ms: u64) {
        if has_remote {
            self.positive_until_ms.insert(
                git_common_dir.to_owned(),
                now_ms.saturating_add(REF_CACHE_TTL_MS),
            );
        } else {
            self.positive_until_ms.remove(git_common_dir);
        }
    }
}

/// Epoch-keyed ref snapshot store dropping the previous snapshot immediately on advance (`38d27db746`).
#[derive(Debug, Default)]
pub struct EpochRefSnapshotCache<T> {
    active_by_common_dir: HashMap<String, (u64, T)>,
}

impl<T> EpochRefSnapshotCache<T> {
    /// Creates an empty [`EpochRefSnapshotCache`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            active_by_common_dir: HashMap::new(),
        }
    }

    /// Stores a new snapshot for `git_common_dir`, immediately dropping any older epoch snapshot (`38d27db746`).
    pub fn replace(&mut self, git_common_dir: &str, snapshot: T) -> u64 {
        let next_epoch = self
            .active_by_common_dir
            .get(git_common_dir)
            .map_or(1, |(epoch, _)| epoch + 1);
        self.active_by_common_dir
            .insert(git_common_dir.to_owned(), (next_epoch, snapshot));
        next_epoch
    }

    /// Returns the active `(epoch, &snapshot)` pair for `git_common_dir`.
    #[must_use]
    pub fn current(&self, git_common_dir: &str) -> Option<(u64, &T)> {
        self.active_by_common_dir
            .get(git_common_dir)
            .map(|(e, s)| (*e, s))
    }

    /// Number of retained snapshots (at most 1 per active `git_common_dir`).
    #[must_use]
    pub fn len(&self) -> usize {
        self.active_by_common_dir.len()
    }

    /// Returns `true` when empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.active_by_common_dir.is_empty()
    }
}

/// Request-scoped memo sharing `remotes`, `config`, and `remoteUrl` git reads within a single
/// branch PR lookup so it executes `<= 9` git processes, invalidated once the forge runs (`2af435185e`).
#[derive(Debug, Default)]
pub struct PrLookupGitMemo {
    /// Whether an external forge provider HTTP/CLI call has already run in this scope.
    pub queried_provider: bool,
    /// Memoized `git remote` list.
    pub remotes: Option<Vec<String>>,
    /// Memoized `git config --get` lookups.
    pub config_values: HashMap<String, Option<String>>,
    /// Memoized `git remote get-url` lookups.
    pub remote_urls: HashMap<String, Option<String>>,
}

impl PrLookupGitMemo {
    /// Returns whether memoized git reads can still be reused (before provider calls).
    #[must_use]
    pub const fn can_reuse(&self) -> bool {
        !self.queried_provider
    }

    /// Marks that an upstream forge provider call ran so post-provider git checks re-read fresh state.
    pub fn mark_provider_queried(&mut self) {
        self.queried_provider = true;
    }
}
