//! Single-flight per-key launch coordination and two-phase SQLite claim helpers for [`ThreadLauncher`](crate::thread_launcher::ThreadLauncher).

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex, OnceLock, Weak};

use legion_contracts::ids::{ProjectId, ThreadId};
use legion_contracts::orchestration::WorkspaceStrategy;
use legion_orchestrator::Orchestrator;
use legion_persistence::thread_launch_store::{
    ClaimThreadLaunchInput, ClaimThreadLaunchOutcome, claim_thread_launch,
    mark_thread_launch_committed, mark_thread_launch_failed, mark_thread_launch_setup_committed,
};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};
use uuid::Uuid;

use crate::outbox_state::utc_now_iso;
use crate::thread_launcher_reuse::{
    validate_existing_thread_compatibility, validate_setup_recovery_compatibility,
};
use crate::thread_launcher_types::{LaunchThreadRequest, LaunchThreadResult, build_resumed_result};
use crate::thread_launcher_workspace::ThreadLaunchError;

type LaunchKeyMap = HashMap<(usize, String), Weak<AsyncMutex<()>>>;

fn launch_key_registry() -> &'static StdMutex<LaunchKeyMap> {
    static REGISTRY: OnceLock<StdMutex<LaunchKeyMap>> = OnceLock::new();
    REGISTRY.get_or_init(|| StdMutex::new(HashMap::new()))
}

/// RAII guard serializing concurrent launches for `(orchestrator_ptr, stable_launch_key)`.
pub(crate) struct LaunchKeyGuard {
    key: (usize, String),
    guard: Option<OwnedMutexGuard<()>>,
    mutex: Option<Arc<AsyncMutex<()>>>,
}

impl Drop for LaunchKeyGuard {
    fn drop(&mut self) {
        drop(self.guard.take());
        drop(self.mutex.take());
        let mut map = launch_key_registry()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if map
            .get(&self.key)
            .is_some_and(|weak| weak.strong_count() == 0)
        {
            map.remove(&self.key);
        }
    }
}

/// Acquires the single-flight async mutex for `(orchestrator_instance, stable_key)`.
pub(crate) async fn acquire_launch_key_lock(
    orchestrator: &Arc<Orchestrator>,
    stable_key: &str,
) -> LaunchKeyGuard {
    let key = (Arc::as_ptr(orchestrator) as usize, stable_key.to_owned());
    let mutex = {
        let mut map = launch_key_registry()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(existing) = map.get(&key).and_then(Weak::upgrade) {
            existing
        } else {
            let created = Arc::new(AsyncMutex::new(()));
            map.insert(key.clone(), Arc::downgrade(&created));
            created
        }
    };
    let guard = Arc::clone(&mutex).lock_owned().await;
    LaunchKeyGuard {
        key,
        guard: Some(guard),
        mutex: Some(mutex),
    }
}

pub(crate) enum StableClaimStep {
    Done(Box<LaunchThreadResult>),
    Continue {
        thread_id: ThreadId,
        recovered_workspace: Option<(Option<String>, Option<String>)>,
    },
}

pub(crate) fn allocate_thread_id(
    explicit: Option<&ThreadId>,
) -> Result<ThreadId, ThreadLaunchError> {
    if let Some(tid) = explicit {
        return Ok(tid.clone());
    }
    ThreadId::new(format!("thread-{}", Uuid::new_v4()))
        .map_err(|e| ThreadLaunchError::Persistence(e.to_string()))
}

pub(crate) fn handle_stable_claim(
    orchestrator: &Orchestrator,
    key: &str,
    req: &LaunchThreadRequest,
    canonical_project_id: &ProjectId,
    now_iso: &str,
) -> Result<StableClaimStep, ThreadLaunchError> {
    let candidate_tid = allocate_thread_id(req.thread_id.as_ref())?;
    let conn = orchestrator
        .sink()
        .db()
        .lock()
        .map_err(|_| ThreadLaunchError::Persistence("database lock poisoned".to_owned()))?;
    let claim = claim_thread_launch(
        &conn,
        &ClaimThreadLaunchInput {
            stable_launch_key: key,
            thread_id: candidate_tid.as_str(),
            project_id: canonical_project_id.as_str(),
            title: &req.title,
            now_iso,
        },
    )
    .map_err(|e| ThreadLaunchError::Persistence(e.to_string()))?;

    match claim {
        ClaimThreadLaunchOutcome::Claimed(_) => Ok(StableClaimStep::Continue {
            thread_id: candidate_tid,
            recovered_workspace: None,
        }),
        ClaimThreadLaunchOutcome::Existing(rec) => {
            if rec.project_id != canonical_project_id.as_str() {
                return Err(ThreadLaunchError::ReuseConflict(format!(
                    "Launch key `{key}` belongs to project `{}`, not `{}`",
                    rec.project_id,
                    canonical_project_id.as_str()
                )));
            }
            let rec_tid = ThreadId::new(&rec.thread_id)
                .map_err(|e| ThreadLaunchError::Persistence(e.to_string()))?;
            if rec.thread_committed {
                return resume_committed_thread(
                    &conn,
                    key,
                    rec_tid,
                    canonical_project_id,
                    &req.workspace_strategy,
                    rec.branch,
                    rec.worktree_path,
                );
            }
            if rec.setup_committed {
                if let Some(detail) = validate_existing_thread_compatibility(
                    &conn,
                    &rec.thread_id,
                    canonical_project_id,
                    &req.workspace_strategy,
                    rec.worktree_path.as_deref(),
                )? {
                    let has_runs = !detail.projection.runs.is_empty();
                    mark_thread_launch_committed(&conn, key, has_runs, &utc_now_iso())
                        .map_err(|e| ThreadLaunchError::Persistence(e.to_string()))?;
                    return Ok(StableClaimStep::Done(Box::new(build_resumed_result(
                        rec_tid,
                        detail.projection,
                        detail.snapshot_sequence,
                        rec.branch,
                        rec.worktree_path,
                    ))));
                }
                validate_setup_recovery_compatibility(&rec, &req.workspace_strategy)?;
                return Ok(StableClaimStep::Continue {
                    thread_id: rec_tid,
                    recovered_workspace: Some((rec.branch, rec.worktree_path)),
                });
            }
            Ok(StableClaimStep::Continue {
                thread_id: rec_tid,
                recovered_workspace: None,
            })
        }
    }
}

fn resume_committed_thread(
    conn: &rusqlite::Connection,
    key: &str,
    rec_tid: ThreadId,
    canonical_project_id: &ProjectId,
    strategy: &WorkspaceStrategy,
    branch: Option<String>,
    worktree_path: Option<String>,
) -> Result<StableClaimStep, ThreadLaunchError> {
    let detail = validate_existing_thread_compatibility(
        conn,
        rec_tid.as_str(),
        canonical_project_id,
        strategy,
        worktree_path.as_deref(),
    )?
    .ok_or_else(|| {
        ThreadLaunchError::ReuseConflict(format!(
            "Committed thread `{}` for launch key `{key}` is missing",
            rec_tid.as_str()
        ))
    })?;
    Ok(StableClaimStep::Done(Box::new(build_resumed_result(
        rec_tid,
        detail.projection,
        detail.snapshot_sequence,
        branch,
        worktree_path,
    ))))
}

pub(crate) fn record_setup_committed(
    orchestrator: &Orchestrator,
    key: &str,
    worktree_path: Option<&str>,
    branch: Option<&str>,
) -> Result<(), ThreadLaunchError> {
    let conn = orchestrator
        .sink()
        .db()
        .lock()
        .map_err(|_| ThreadLaunchError::Persistence("database lock poisoned".to_owned()))?;
    mark_thread_launch_setup_committed(&conn, key, worktree_path, branch, &utc_now_iso())
        .map_err(|e| ThreadLaunchError::Persistence(e.to_string()))
}

pub(crate) fn record_thread_committed(
    orchestrator: &Orchestrator,
    key: &str,
    message_committed: bool,
) -> Result<(), ThreadLaunchError> {
    let conn = orchestrator
        .sink()
        .db()
        .lock()
        .map_err(|_| ThreadLaunchError::Persistence("database lock poisoned".to_owned()))?;
    mark_thread_launch_committed(&conn, key, message_committed, &utc_now_iso())
        .map_err(|e| ThreadLaunchError::Persistence(e.to_string()))
}

pub(crate) fn record_launch_failure(orchestrator: &Orchestrator, key: &str, message: &str) {
    if let Ok(conn) = orchestrator.sink().db().lock() {
        let _ = mark_thread_launch_failed(&conn, key, message, &utc_now_iso());
    }
}
