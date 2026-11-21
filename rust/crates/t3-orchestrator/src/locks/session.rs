//! Compile-time typestate [`LockSession`] enforcing lock rank progression.

use std::marker::PhantomData;

use t3_contracts::ids::ThreadId;
use t3_shared::concurrency::KeyedLockGuard;

use crate::locks::OrchestratorLocks;
use crate::locks::types::{
    CanAcquire, ChildThreadRank, DeletionRank, LockError, LockRank, LockRankMarker,
    MessageIntakeRank, ParentThreadRank,
};

/// Compile-time rank-checked RAII lock session holding acquired guards in rank order.
pub struct LockSession<'a, R: LockRankMarker> {
    pub(crate) locks: &'a OrchestratorLocks,
    pub(crate) deletion_guard: Option<KeyedLockGuard<()>>,
    pub(crate) intake_guard: Option<KeyedLockGuard<ThreadId>>,
    pub(crate) parent_guard: Option<KeyedLockGuard<ThreadId>>,
    pub(crate) child_guards: Vec<KeyedLockGuard<ThreadId>>,
    pub(crate) held_dispatch_threads: Vec<ThreadId>,
    pub(crate) rank_marker: PhantomData<R>,
}

impl<'a, R: LockRankMarker> LockSession<'a, R> {
    /// Returns the highest [`LockRank`] currently held by this session.
    pub const fn current_rank(&self) -> Option<LockRank> {
        R::RANK
    }

    /// Acquires [`LockRank::DeletionGuard`] if statically allowed from rank `R`.
    pub async fn acquire_deletion_guard(self) -> Result<LockSession<'a, DeletionRank>, LockError>
    where
        R: CanAcquire<DeletionRank>,
    {
        let guard = self.locks.deletion_guard.acquire(()).await;
        Ok(LockSession {
            locks: self.locks,
            deletion_guard: Some(guard),
            intake_guard: self.intake_guard,
            parent_guard: self.parent_guard,
            child_guards: self.child_guards,
            held_dispatch_threads: self.held_dispatch_threads,
            rank_marker: PhantomData,
        })
    }

    /// Acquires [`LockRank::MessageIntake`] for `thread_id` if allowed from rank `R`.
    pub async fn acquire_message_intake(
        self,
        thread_id: ThreadId,
    ) -> Result<LockSession<'a, MessageIntakeRank>, LockError>
    where
        R: CanAcquire<MessageIntakeRank>,
    {
        let guard = self.locks.message_intake.acquire(thread_id).await;
        Ok(LockSession {
            locks: self.locks,
            deletion_guard: self.deletion_guard,
            intake_guard: Some(guard),
            parent_guard: self.parent_guard,
            child_guards: self.child_guards,
            held_dispatch_threads: self.held_dispatch_threads,
            rank_marker: PhantomData,
        })
    }

    /// Acquires [`LockRank::ParentThread`] for `thread_id` if allowed from rank `R`.
    pub async fn acquire_parent_thread(
        mut self,
        thread_id: ThreadId,
    ) -> Result<LockSession<'a, ParentThreadRank>, LockError>
    where
        R: CanAcquire<ParentThreadRank>,
    {
        if self.held_dispatch_threads.contains(&thread_id) {
            return Err(LockError::ReentrantThreadLock { thread_id });
        }
        let guard = self.locks.thread_dispatch.acquire(thread_id.clone()).await;
        self.held_dispatch_threads.push(thread_id);
        Ok(LockSession {
            locks: self.locks,
            deletion_guard: self.deletion_guard,
            intake_guard: self.intake_guard,
            parent_guard: Some(guard),
            child_guards: self.child_guards,
            held_dispatch_threads: self.held_dispatch_threads,
            rank_marker: PhantomData,
        })
    }

    /// Acquires [`LockRank::ChildThread`] for `thread_id` if allowed from rank `R`.
    pub async fn acquire_child_thread(
        self,
        thread_id: ThreadId,
    ) -> Result<LockSession<'a, ChildThreadRank>, LockError>
    where
        R: CanAcquire<ChildThreadRank>,
    {
        self.acquire_child_threads([thread_id]).await
    }

    /// Acquires [`LockRank::ChildThread`] for multiple `thread_ids` in sorted canonical order.
    pub async fn acquire_child_threads(
        mut self,
        thread_ids: impl IntoIterator<Item = ThreadId>,
    ) -> Result<LockSession<'a, ChildThreadRank>, LockError>
    where
        R: CanAcquire<ChildThreadRank>,
    {
        let mut sorted: Vec<ThreadId> = thread_ids.into_iter().collect();
        sorted.sort();
        sorted.dedup();
        for tid in &sorted {
            if self.held_dispatch_threads.contains(tid) {
                return Err(LockError::ReentrantThreadLock {
                    thread_id: tid.clone(),
                });
            }
        }
        for tid in sorted {
            let guard = self.locks.thread_dispatch.acquire(tid.clone()).await;
            self.held_dispatch_threads.push(tid);
            self.child_guards.push(guard);
        }
        Ok(LockSession {
            locks: self.locks,
            deletion_guard: self.deletion_guard,
            intake_guard: self.intake_guard,
            parent_guard: self.parent_guard,
            child_guards: self.child_guards,
            held_dispatch_threads: self.held_dispatch_threads,
            rank_marker: PhantomData,
        })
    }
}
