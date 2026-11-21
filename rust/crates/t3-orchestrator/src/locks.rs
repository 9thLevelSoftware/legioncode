//! Typed rank-enforced lock hierarchy for thread command execution.
//!
//! Mirrors `apps/server/src/orchestration-v2/ThreadCommandExecutor.ts` and
//! `Orchestrator.ts:16262-16326`, enforcing the strict acquisition order:
//! `DeletionGuard -> MessageIntake -> ParentThread -> ChildThread`.
//!
//! # Examples
//!
//! ```
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use t3_contracts::ids::ThreadId;
//! use t3_orchestrator::locks::{LockError, LockRank, OrchestratorLocks};
//!
//! let locks = OrchestratorLocks::new();
//! let parent = ThreadId::new("thread-parent")?;
//! let child = ThreadId::new("thread-child")?;
//!
//! {
//!     let s0 = locks.session();
//!     let s1 = s0.acquire_deletion_guard().await?;
//!     let s2 = s1.acquire_message_intake(child.clone()).await?;
//!     let s3 = s2.acquire_parent_thread(parent.clone()).await?;
//!     let _s4 = s3.acquire_child_thread(child.clone()).await?;
//! }
//!
//! let res = locks
//!     .with_child_thread(child.clone(), async {
//!         locks.with_parent_thread(parent.clone(), async { Ok::<(), LockError>(()) }).await
//!     })
//!     .await;
//! assert!(matches!(
//!     res,
//!     Err(LockError::RankInversion {
//!         held: LockRank::ChildThread,
//!         requested: LockRank::ParentThread,
//!     })
//! ));
//! # Ok(())
//! # }
//! ```

pub mod session;
pub mod types;

use std::cell::RefCell;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;

use t3_contracts::ids::ThreadId;
use t3_shared::concurrency::{KeyedLock, KeyedLockGuard};

pub use session::LockSession;
pub use types::{
    CanAcquire, ChildThreadRank, DeletionRank, LockError, LockRank, LockRankMarker,
    MessageIntakeRank, ParentThreadRank, Unlocked,
};
use types::{HELD_LOCK_STACK, HeldEntry, StackPopGuard, validate_stack_push};

/// Rank-enforced lock manager backed by [`KeyedLock`] instances.
#[derive(Clone, Default)]
pub struct OrchestratorLocks {
    pub(crate) deletion_guard: KeyedLock<()>,
    pub(crate) message_intake: KeyedLock<ThreadId>,
    pub(crate) thread_dispatch: KeyedLock<ThreadId>,
}

impl std::fmt::Debug for OrchestratorLocks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OrchestratorLocks")
            .field("deletion_active", &self.deletion_guard.active_key_count())
            .field(
                "message_intake_active",
                &self.message_intake.active_key_count(),
            )
            .field(
                "thread_dispatch_active",
                &self.thread_dispatch.active_key_count(),
            )
            .finish()
    }
}

impl OrchestratorLocks {
    /// Creates a new [`OrchestratorLocks`] hierarchy.
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts a compile-time rank-checked [`LockSession`] in the [`Unlocked`] state.
    pub fn session(&self) -> LockSession<'_, Unlocked> {
        LockSession {
            locks: self,
            deletion_guard: None,
            intake_guard: None,
            parent_guard: None,
            child_guards: Vec::new(),
            held_dispatch_threads: Vec::new(),
            rank_marker: PhantomData,
        }
    }

    /// Runs `fut` while holding the global [`LockRank::DeletionGuard`] lock.
    pub async fn with_deletion_guard<F, T, E>(&self, fut: F) -> Result<T, E>
    where
        F: Future<Output = Result<T, E>>,
        E: From<LockError>,
    {
        let entry = HeldEntry {
            rank: LockRank::DeletionGuard,
            thread_id: None,
        };
        self.run_ranked(entry, self.deletion_guard.acquire(()), fut)
            .await
    }

    /// Runs `fut` while holding the [`LockRank::MessageIntake`] lock for `thread_id`.
    pub async fn with_message_intake<F, T, E>(&self, thread_id: ThreadId, fut: F) -> Result<T, E>
    where
        F: Future<Output = Result<T, E>>,
        E: From<LockError>,
    {
        let entry = HeldEntry {
            rank: LockRank::MessageIntake,
            thread_id: Some(thread_id.clone()),
        };
        self.run_ranked(entry, self.message_intake.acquire(thread_id), fut)
            .await
    }

    /// Runs `fut` while holding the [`LockRank::ParentThread`] lock for `thread_id`.
    pub async fn with_parent_thread<F, T, E>(&self, thread_id: ThreadId, fut: F) -> Result<T, E>
    where
        F: Future<Output = Result<T, E>>,
        E: From<LockError>,
    {
        let entry = HeldEntry {
            rank: LockRank::ParentThread,
            thread_id: Some(thread_id.clone()),
        };
        self.run_ranked(entry, self.thread_dispatch.acquire(thread_id), fut)
            .await
    }

    /// Runs `fut` while holding the [`LockRank::ChildThread`] lock for `thread_id`.
    pub async fn with_child_thread<F, T, E>(&self, thread_id: ThreadId, fut: F) -> Result<T, E>
    where
        F: Future<Output = Result<T, E>>,
        E: From<LockError>,
    {
        let entry = HeldEntry {
            rank: LockRank::ChildThread,
            thread_id: Some(thread_id.clone()),
        };
        self.run_ranked(entry, self.thread_dispatch.acquire(thread_id), fut)
            .await
    }

    /// Runs `fut` while holding [`LockRank::ChildThread`] locks for `thread_ids` in sorted order.
    pub async fn with_child_threads<F, T, E>(
        &self,
        thread_ids: impl IntoIterator<Item = ThreadId>,
        fut: F,
    ) -> Result<T, E>
    where
        F: Future<Output = Result<T, E>>,
        E: From<LockError>,
    {
        let mut sorted: Vec<ThreadId> = thread_ids.into_iter().collect();
        sorted.sort();
        sorted.dedup();
        self.with_sorted_children(&sorted, fut).await
    }

    fn with_sorted_children<'a, F, T, E>(
        &'a self,
        remaining: &'a [ThreadId],
        fut: F,
    ) -> Pin<Box<dyn Future<Output = Result<T, E>> + 'a>>
    where
        F: Future<Output = Result<T, E>> + 'a,
        E: From<LockError> + 'a,
        T: 'a,
    {
        Box::pin(async move {
            let Some((first, rest)) = remaining.split_first() else {
                return fut.await;
            };
            self.with_child_thread(first.clone(), self.with_sorted_children(rest, fut))
                .await
        })
    }

    async fn run_ranked<K, F, T, E>(
        &self,
        entry: HeldEntry,
        acquire_fut: impl Future<Output = KeyedLockGuard<K>>,
        body: F,
    ) -> Result<T, E>
    where
        K: Eq + std::hash::Hash + Clone,
        F: Future<Output = Result<T, E>>,
        E: From<LockError>,
    {
        let existing_stack = HELD_LOCK_STACK.try_with(|cell| {
            let stack = cell.borrow();
            validate_stack_push(&stack, &entry)
        });
        match existing_stack {
            Ok(Err(err)) => Err(err.into()),
            Ok(Ok(())) => {
                let _guard = acquire_fut.await;
                let _pop = StackPopGuard::push(&entry);
                body.await
            }
            Err(_) => {
                let _guard = acquire_fut.await;
                HELD_LOCK_STACK.scope(RefCell::new(vec![entry]), body).await
            }
        }
    }
}
