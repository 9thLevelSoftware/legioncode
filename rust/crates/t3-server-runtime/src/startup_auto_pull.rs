//! Startup automatic project `git pull --ff-only` runner awaiting in-flight status fetches (`#39915`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, Notify};
use tokio::task::JoinHandle;

use crate::git_checkpoints_cli::run_git;

/// Default timeout (`5s`) after which non-startup status polls read last-fetched refs (`#39915`).
pub const STATUS_FETCH_SOFT_WAIT: Duration = Duration::from_secs(5);

/// Per-project outcome recorded by [`auto_pull_projects`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutoPullOutcome {
    /// Startup auto-pull skipped because auto-pull is disabled for the project.
    SkippedDisabled,
    /// Skipped because the workspace has no upstream tracking branch or is dirty.
    SkippedNotCleanOrNoUpstream,
    /// Already up to date with upstream after awaiting status fetch.
    AlreadyUpToDate,
    /// Fast-forwarded cleanly after awaiting status fetch.
    FastForwarded,
    /// Fetch or fast-forward failed (never aborts remaining projects).
    Failed(String),
}

/// Target repository configuration passed to [`auto_pull_projects`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoPullProjectSpec {
    pub project_id: String,
    pub workspace_root: PathBuf,
    pub auto_pull: bool,
}

/// Options controlling whether status reads wait for an active slow fetch (`#39915`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GitStatusFetchOptions {
    /// When `true` (used at startup), awaits the full in-flight fetch handle rather than
    /// falling back to cached refs after [`STATUS_FETCH_SOFT_WAIT`].
    pub await_status_fetch: bool,
}

/// Shared coordinator tracking active slow background `git fetch` jobs keyed by `(common_dir, remote)`.
#[derive(Debug, Clone, Default)]
pub struct StatusFetchCoordinator {
    in_flight: Arc<Mutex<BTreeMap<String, Arc<InFlightFetch>>>>,
}

#[derive(Debug)]
struct InFlightFetch {
    done: std::sync::atomic::AtomicBool,
    notify: Notify,
}

impl StatusFetchCoordinator {
    /// Construct an empty [`StatusFetchCoordinator`].
    pub fn new() -> Self {
        Self::default()
    }

    fn key(cwd: &Path, remote: &str) -> String {
        format!("{}\0{remote}", cwd.to_string_lossy())
    }

    /// Registers an asynchronous background fetch task for `(cwd, remote)` and returns its join handle.
    pub async fn start_fetch<F>(&self, cwd: &Path, remote: &str, fetch_op: F) -> JoinHandle<()>
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        let key = Self::key(cwd, remote);
        let entry = Arc::new(InFlightFetch {
            done: std::sync::atomic::AtomicBool::new(false),
            notify: Notify::new(),
        });
        {
            let mut guard = self.in_flight.lock().await;
            guard.insert(key.clone(), Arc::clone(&entry));
        }
        let registry = Arc::clone(&self.in_flight);
        tokio::spawn(async move {
            fetch_op.await;
            entry.done.store(true, std::sync::atomic::Ordering::Release);
            entry.notify.notify_waiters();
            let mut guard = registry.lock().await;
            guard.remove(&key);
        })
    }

    /// Waits for an active fetch on `(cwd, remote)`.
    ///
    /// When `opts.await_status_fetch` is `true` (startup auto-pull), awaits completion of any
    /// slow running fetch. Otherwise returns once `soft_wait` elapses (`#39915`).
    pub async fn wait_for_pending_fetch(
        &self,
        cwd: &Path,
        remote: &str,
        opts: GitStatusFetchOptions,
        soft_wait: Duration,
    ) -> bool {
        let pending = {
            let guard = self.in_flight.lock().await;
            guard.get(&Self::key(cwd, remote)).cloned()
        };
        let Some(entry) = pending else {
            return true;
        };
        if entry.done.load(std::sync::atomic::Ordering::Acquire) {
            return true;
        }
        let wait_fut = async {
            while !entry.done.load(std::sync::atomic::Ordering::Acquire) {
                entry.notify.notified().await;
            }
        };
        if opts.await_status_fetch {
            wait_fut.await;
            true
        } else {
            tokio::time::timeout(soft_wait, wait_fut).await.is_ok()
        }
    }
}

/// Runs startup automatic `git pull --ff-only` for eligible projects, awaiting any slow in-flight
/// fetch via [`GitStatusFetchOptions { await_status_fetch: true }`] (`#39915`).
pub async fn auto_pull_projects(
    projects: &[AutoPullProjectSpec],
    default_auto_pull: bool,
    coordinator: &StatusFetchCoordinator,
) -> Vec<(String, AutoPullOutcome)> {
    let mut results = Vec::with_capacity(projects.len());
    for project in projects {
        if !(project.auto_pull && default_auto_pull) {
            results.push((project.project_id.clone(), AutoPullOutcome::SkippedDisabled));
            continue;
        }
        let _ = coordinator
            .wait_for_pending_fetch(
                &project.workspace_root,
                "origin",
                GitStatusFetchOptions {
                    await_status_fetch: true,
                },
                STATUS_FETCH_SOFT_WAIT,
            )
            .await;

        let cwd = project.workspace_root.clone();
        let outcome = tokio::task::spawn_blocking(move || pull_project_if_behind(&cwd))
            .await
            .unwrap_or_else(|err| AutoPullOutcome::Failed(err.to_string()));
        results.push((project.project_id.clone(), outcome));
    }
    results
}

fn pull_project_if_behind(cwd: &Path) -> AutoPullOutcome {
    let Ok(upstream_bytes) = run_git(
        cwd,
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
        &[],
    ) else {
        return AutoPullOutcome::SkippedNotCleanOrNoUpstream;
    };
    if String::from_utf8_lossy(&upstream_bytes).trim().is_empty() {
        return AutoPullOutcome::SkippedNotCleanOrNoUpstream;
    }

    let Ok(status_bytes) = run_git(cwd, &["status", "--porcelain"], &[]) else {
        return AutoPullOutcome::SkippedNotCleanOrNoUpstream;
    };
    if !String::from_utf8_lossy(&status_bytes).trim().is_empty() {
        return AutoPullOutcome::SkippedNotCleanOrNoUpstream;
    }

    if let Err(err) = run_git(cwd, &["fetch", "--quiet"], &[]) {
        return AutoPullOutcome::Failed(err.to_string());
    }

    let Ok(counts_bytes) = run_git(
        cwd,
        &["rev-list", "--left-right", "--count", "HEAD...@{u}"],
        &[],
    ) else {
        return AutoPullOutcome::SkippedNotCleanOrNoUpstream;
    };
    let counts = String::from_utf8_lossy(&counts_bytes);
    let mut parts = counts.split_whitespace();
    let ahead: u64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let behind: u64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    if behind == 0 || ahead > 0 {
        return AutoPullOutcome::AlreadyUpToDate;
    }

    match run_git(cwd, &["merge", "--ff-only", "@{u}"], &[]) {
        Ok(_) => AutoPullOutcome::FastForwarded,
        Err(err) => AutoPullOutcome::Failed(err.to_string()),
    }
}
