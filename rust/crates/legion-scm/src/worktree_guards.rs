//! Shared-checkout ref-switch guards (`#38271`), remote-prefixed branch publishing
//! (`#40877`), retryable clone claims (`#40739`), race-safe task-folder rollback (`#41061`),
//! pre-occupied checkout hook detection (`#40419`), and clean default-branch pushes (`#40538`).

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use crate::workspace_folders_parse::{is_within_root, lexical_clean};

/// Git operation identifier emitted when `createWorktree` fails specifically because branch
/// creation failed (e.g., another concurrent task-folder creation won the branch race, `#41061`).
pub const CREATE_WORKTREE_BRANCH_OPERATION: &str = "GitVcsDriver.createWorktree.createBranch";

/// Minimal thread metadata inspected by [`busy_threads_in_checkout`] (`#38271`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutThreadRef {
    /// Unique thread identifier.
    pub id: String,
    /// Display title shown in refusal errors.
    pub title: String,
    /// Owning project ID.
    pub project_id: String,
    /// Linked worktree path, or `None` when running in the project's shared root checkout.
    pub worktree_path: Option<String>,
    /// True when the thread currently has an active turn/run.
    pub is_busy: bool,
}

/// Returns active threads currently executing inside `checkout_paths` or their shared
/// `project_ids` (for main-checkout threads where `worktree_path` is `None`) (`#38271`).
#[must_use]
pub fn busy_threads_in_checkout<'a, S: ::std::hash::BuildHasher>(
    checkout_paths: &[&str],
    project_ids: &HashSet<String, S>,
    threads: &'a [CheckoutThreadRef],
) -> Vec<&'a CheckoutThreadRef> {
    threads
        .iter()
        .filter(|thread| {
            thread.is_busy
                && match &thread.worktree_path {
                    None => project_ids.contains(&thread.project_id),
                    Some(wt) => checkout_paths.iter().any(|p| p == wt),
                }
        })
        .collect()
}

/// Formats the user-facing refusal message naming active threads in a shared checkout (`#38271`).
#[must_use]
pub fn describe_busy_checkout(titles: &[&str]) -> String {
    let quoted: Vec<String> = titles.iter().map(|t| format!("'{t}'")).collect();
    let names = match quoted.len() {
        0 => String::new(),
        1 | 2 => quoted.join(" and "),
        n => format!("{}, {} and {} more", quoted[0], quoted[1], n - 2),
    };
    let (verb, pronoun) = if quoted.len() == 1 {
        ("is", "it")
    } else {
        ("are", "them")
    };
    format!(
        "{names} {verb} still working in this checkout. Wait for {pronoun} to finish, or use a worktree for a separate branch."
    )
}

/// Refuses shared checkout ref switches (`switchRef`, `createRef(switchRef)`, local PR checkout)
/// whenever any non-idle active or archived thread shares the checkout (`#38271`).
///
/// # Errors
///
/// Returns `Err(detail)` describing the blocking active thread(s) when non-empty.
pub fn assert_checkout_idle<S: ::std::hash::BuildHasher>(
    checkout_paths: &[&str],
    project_ids: &HashSet<String, S>,
    threads: &[CheckoutThreadRef],
) -> Result<(), String> {
    let busy = busy_threads_in_checkout(checkout_paths, project_ids, threads);
    let mut seen_ids = HashSet::new();
    let mut titles = Vec::new();
    for t in busy {
        if seen_ids.insert(t.id.as_str()) {
            titles.push(t.title.as_str());
        }
    }
    if titles.is_empty() {
        Ok(())
    } else {
        Err(describe_busy_checkout(&titles))
    }
}

/// Splits `<remote>/<branch>` using the longest matching known remote name (`#40877`).
#[must_use]
pub fn parse_branch_with_remote_names(
    branch_name: &str,
    remote_names: &[&str],
) -> Option<(String, String)> {
    let mut sorted: Vec<&str> = remote_names
        .iter()
        .copied()
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .collect();
    sorted.sort_by_key(|r| std::cmp::Reverse(r.len()));

    for remote in sorted {
        if let Some(rest) = branch_name.strip_prefix(remote)
            && let Some(branch) = rest.strip_prefix('/')
            && !branch.is_empty()
        {
            return Some((remote.to_owned(), branch.to_owned()));
        }
    }
    None
}

/// Resolves the target publish branch name when pushing `branch_name` to `destination_remote` (`#40877`).
///
/// Strips ONLY `destination_remote/` so a local branch prefixed with another remote's name
/// (e.g., `alice/fix` pushed to `origin`) publishes under its full name instead of truncating to `fix`.
#[must_use]
pub fn resolve_publish_branch_name(branch_name: &str, destination_remote: &str) -> String {
    let trimmed_remote = destination_remote.trim();
    if trimmed_remote.is_empty() {
        return branch_name.to_owned();
    }
    parse_branch_with_remote_names(branch_name, &[trimmed_remote])
        .map_or_else(|| branch_name.to_owned(), |(_, stripped)| stripped)
}

/// Lifecycle phase of a tracked repository clone operation (`#40739`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClonePhase {
    /// Actively cloning into the destination directory.
    Running,
    /// Completed successfully.
    Succeeded,
    /// Failed before completion.
    Failed,
    /// Cancelled by the caller.
    Cancelled,
}

/// Tracked clone entry held by [`ProjectCloneTracker`] (`#40739`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackedCloneEntry {
    /// Project ID requesting the clone.
    pub project_id: String,
    /// Target filesystem directory path.
    pub destination_path: String,
    /// Current lifecycle phase.
    pub phase: ClonePhase,
    /// Monotonic claim sequence counter.
    pub sequence: u64,
}

/// Tracks active and retained repository clones so failed or cancelled clones never block
/// subsequent re-clone attempts for the same project or destination (`#40739`).
#[derive(Debug, Default)]
pub struct ProjectCloneTracker {
    sequence: u64,
    by_project: BTreeMap<String, TrackedCloneEntry>,
}

impl ProjectCloneTracker {
    /// Creates an empty [`ProjectCloneTracker`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Attempts to claim `destination_path` for `project_id` (`#40739`).
    ///
    /// Rejects ONLY if an overlapping entry for the same `project_id` or `destination_path`
    /// still has `phase == ClonePhase::Running`. Returns superseded non-running project IDs
    /// whose retention cleanup timers should be cancelled before starting the new clone.
    ///
    /// # Errors
    ///
    /// Returns `Err` when a running clone already owns the project or destination.
    pub fn try_claim(
        &mut self,
        project_id: &str,
        destination_path: &str,
    ) -> Result<Vec<String>, String> {
        let overlapping: Vec<TrackedCloneEntry> = self
            .by_project
            .values()
            .filter(|entry| {
                entry.project_id == project_id || entry.destination_path == destination_path
            })
            .cloned()
            .collect();
        if overlapping
            .iter()
            .any(|entry| entry.phase == ClonePhase::Running)
        {
            return Err("A clone into this destination is already in progress.".to_owned());
        }

        let mut superseded = Vec::with_capacity(overlapping.len());
        for stale in overlapping {
            self.by_project.remove(&stale.project_id);
            superseded.push(stale.project_id);
        }
        self.sequence = self.sequence.saturating_add(1);
        self.by_project.insert(
            project_id.to_owned(),
            TrackedCloneEntry {
                project_id: project_id.to_owned(),
                destination_path: destination_path.to_owned(),
                phase: ClonePhase::Running,
                sequence: self.sequence,
            },
        );
        Ok(superseded)
    }

    /// Transitions an active clone to `Succeeded`, `Failed`, or `Cancelled` (`#40739`).
    pub fn set_phase(&mut self, project_id: &str, phase: ClonePhase) {
        if let Some(entry) = self.by_project.get_mut(project_id) {
            entry.phase = phase;
        }
    }
}

/// Tracks repositories lacking the target branch before a task-folder creation attempt so rollback
/// deletes only branches this attempt created and NEVER deletes a concurrent winner's branch (`#41061`).
#[derive(Debug, Default)]
pub struct TaskBranchCreationGuard {
    branch_missing_in: HashSet<String>,
    branch_made_in: Vec<String>,
}

impl TaskBranchCreationGuard {
    /// Initializes a guard from the repository `cwd` paths that did NOT already have `branch`
    /// prior to creating worktrees (`#41061`).
    #[must_use]
    pub fn new(missing_in_cwds: impl IntoIterator<Item = String>) -> Self {
        Self {
            branch_missing_in: missing_in_cwds.into_iter().collect(),
            branch_made_in: Vec::new(),
        }
    }

    /// Records the exit outcome of a `createWorktree` attempt for `cwd` (`#41061`).
    ///
    /// When `createWorktree` failed with `operation == CREATE_WORKTREE_BRANCH_OPERATION`,
    /// Git refused branch creation (because another task creation created it first), so `cwd`
    /// is NOT added to [`Self::branches_to_rollback`].
    pub fn record_outcome(&mut self, cwd: &str, succeeded: bool, failed_operation: Option<&str>) {
        let refused = !succeeded && failed_operation == Some(CREATE_WORKTREE_BRANCH_OPERATION);
        if self.branch_missing_in.contains(cwd)
            && !refused
            && !self.branch_made_in.iter().any(|p| p == cwd)
        {
            self.branch_made_in.push(cwd.to_owned());
        }
    }

    /// Returns repository `cwd` paths whose created branch should be cleaned up on rollback (`#41061`).
    #[must_use]
    pub fn branches_to_rollback(&self) -> &[String] {
        &self.branch_made_in
    }
}

/// Returns `true` when `target` is already occupied by a symlink (even dangling), non-directory
/// file, or non-empty directory created by a checkout hook so worktree creation should skip it (`#40419`).
#[must_use]
pub fn is_checkout_slot_preoccupied(target: &Path) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(target) else {
        return false;
    };
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return true;
    }
    std::fs::read_dir(target).map_or(true, |mut entries| entries.next().is_some())
}

/// Returns `true` when `repo_path` sits inside any already-skipped outer repository path (`#40419`).
#[must_use]
pub fn should_skip_nested_repository(repo_path: &Path, skipped_outer: &[PathBuf]) -> bool {
    let clean_repo = lexical_clean(repo_path);
    skipped_outer
        .iter()
        .any(|outer| is_within_root(&clean_repo, outer))
}

/// Destination state evaluated when checking whether pushing a clean branch is a no-op (`#40538`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushDestinationCheck {
    /// No upstream/push destination configured.
    None,
    /// Branch tracks a local branch (`branch.<name>.remote = .`), so clean delta does not guarantee remote is up to date.
    LocalUpstreamDot,
    /// Separate push remote configured (`branch.<name>.pushRemote` or `remote.pushDefault`).
    SeparatePushRemote,
    /// Pushes directly to its fetch upstream remote (`origin`, etc.).
    MatchesUpstreamRemote {
        /// True when fetch URL equals push URL.
        pushes_only_to_fetch_url: bool,
    },
}

/// Returns `true` when pushing a clean branch with an upstream can safely short-circuit as
/// `skipped_up_to_date` (`#40538`: a `.` local upstream never short-circuits).
#[must_use]
pub const fn should_skip_push_as_up_to_date(
    has_no_local_delta: bool,
    has_upstream: bool,
    destination: PushDestinationCheck,
) -> bool {
    if !has_no_local_delta || !has_upstream {
        return false;
    }
    match destination {
        PushDestinationCheck::None => true,
        PushDestinationCheck::LocalUpstreamDot | PushDestinationCheck::SeparatePushRemote => false,
        PushDestinationCheck::MatchesUpstreamRemote {
            pushes_only_to_fetch_url,
        } => pushes_only_to_fetch_url,
    }
}
