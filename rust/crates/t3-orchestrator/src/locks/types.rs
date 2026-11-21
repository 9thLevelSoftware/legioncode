//! Lock ranks, error types, compile-time rank markers, and task-local runtime guard.

use std::cell::RefCell;

use t3_contracts::ids::ThreadId;
use thiserror::Error;

/// Canonical rank ordering of orchestration locks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LockRank {
    /// Global deletion guard serializing thread deletion source-retention planning.
    DeletionGuard = 0,
    /// Per-thread message intake queue preserving FIFO arrival order before lineage reads.
    MessageIntake = 1,
    /// Parent/source thread dispatch lock taken before child/target thread locks.
    ParentThread = 2,
    /// Leaf/target thread dispatch lock.
    ChildThread = 3,
}

/// Errors returned when a lock acquisition would violate rank order or reenter a thread lock.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LockError {
    /// Attempted to acquire a lock whose rank is not strictly higher than the currently held lock.
    #[error("lock rank inversion: cannot acquire {requested:?} while holding {held:?}")]
    RankInversion {
        /// Highest lock rank currently held on the task/session.
        held: LockRank,
        /// Lower or equal rank requested.
        requested: LockRank,
    },
    /// Attempted to acquire `ChildThread` locks out of canonical ascending [`ThreadId`] order.
    #[error("child thread lock order violation: '{requested}' must be greater than '{held}'")]
    ChildKeyOrder {
        /// Previously locked child thread ID.
        held: ThreadId,
        /// Requested child thread ID that is `<= held`.
        requested: ThreadId,
    },
    /// Attempted to reacquire the shared thread dispatch lock for an already-locked [`ThreadId`].
    #[error("reentrant thread dispatch lock rejected for thread '{thread_id}'")]
    ReentrantThreadLock {
        /// Thread ID already held by an enclosing parent or child lock.
        thread_id: ThreadId,
    },
}

mod sealed {
    pub trait Sealed {}
}

/// Marker trait for compile-time lock rank states in [`crate::locks::LockSession`].
pub trait LockRankMarker: sealed::Sealed {
    /// Rank associated with this state, or `None` when unlocked.
    const RANK: Option<LockRank>;
}

/// Initial unlocked state for [`crate::locks::LockSession`].
#[derive(Debug)]
pub struct Unlocked;
/// Rank state after acquiring [`LockRank::DeletionGuard`].
#[derive(Debug)]
pub struct DeletionRank;
/// Rank state after acquiring [`LockRank::MessageIntake`].
#[derive(Debug)]
pub struct MessageIntakeRank;
/// Rank state after acquiring [`LockRank::ParentThread`].
#[derive(Debug)]
pub struct ParentThreadRank;
/// Rank state after acquiring [`LockRank::ChildThread`].
#[derive(Debug)]
pub struct ChildThreadRank;

impl sealed::Sealed for Unlocked {}
impl sealed::Sealed for DeletionRank {}
impl sealed::Sealed for MessageIntakeRank {}
impl sealed::Sealed for ParentThreadRank {}
impl sealed::Sealed for ChildThreadRank {}

impl LockRankMarker for Unlocked {
    const RANK: Option<LockRank> = None;
}
impl LockRankMarker for DeletionRank {
    const RANK: Option<LockRank> = Some(LockRank::DeletionGuard);
}
impl LockRankMarker for MessageIntakeRank {
    const RANK: Option<LockRank> = Some(LockRank::MessageIntake);
}
impl LockRankMarker for ParentThreadRank {
    const RANK: Option<LockRank> = Some(LockRank::ParentThread);
}
impl LockRankMarker for ChildThreadRank {
    const RANK: Option<LockRank> = Some(LockRank::ChildThread);
}

/// Compile-time capability witness allowing progression from rank `Self` to `Next`.
pub trait CanAcquire<Next: LockRankMarker>: LockRankMarker {}

impl CanAcquire<DeletionRank> for Unlocked {}
impl CanAcquire<MessageIntakeRank> for Unlocked {}
impl CanAcquire<ParentThreadRank> for Unlocked {}
impl CanAcquire<ChildThreadRank> for Unlocked {}
impl CanAcquire<MessageIntakeRank> for DeletionRank {}
impl CanAcquire<ParentThreadRank> for DeletionRank {}
impl CanAcquire<ChildThreadRank> for DeletionRank {}
impl CanAcquire<ParentThreadRank> for MessageIntakeRank {}
impl CanAcquire<ChildThreadRank> for MessageIntakeRank {}
impl CanAcquire<ChildThreadRank> for ParentThreadRank {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HeldEntry {
    pub(crate) rank: LockRank,
    pub(crate) thread_id: Option<ThreadId>,
}

tokio::task_local! {
    pub(crate) static HELD_LOCK_STACK: RefCell<Vec<HeldEntry>>;
}

pub(crate) struct StackPopGuard(bool);

impl StackPopGuard {
    pub(crate) fn push(entry: &HeldEntry) -> Self {
        let pushed = HELD_LOCK_STACK
            .try_with(|cell| cell.borrow_mut().push(entry.clone()))
            .is_ok();
        Self(pushed)
    }
}

impl Drop for StackPopGuard {
    fn drop(&mut self) {
        if self.0 {
            let _ = HELD_LOCK_STACK.try_with(|cell| {
                let _ = cell.borrow_mut().pop();
            });
        }
    }
}

pub(crate) fn validate_stack_push(stack: &[HeldEntry], next: &HeldEntry) -> Result<(), LockError> {
    if let (Some(next_tid), true) = (
        next.thread_id.as_ref(),
        matches!(next.rank, LockRank::ParentThread | LockRank::ChildThread),
    ) && stack.iter().any(|held| {
        matches!(held.rank, LockRank::ParentThread | LockRank::ChildThread)
            && held.thread_id.as_ref() == Some(next_tid)
    }) {
        return Err(LockError::ReentrantThreadLock {
            thread_id: next_tid.clone(),
        });
    }

    if let Some(last) = stack.last() {
        if next.rank < last.rank {
            return Err(LockError::RankInversion {
                held: last.rank,
                requested: next.rank,
            });
        }
        if next.rank == last.rank {
            if next.rank == LockRank::ChildThread
                && let (Some(held_tid), Some(req_tid)) =
                    (last.thread_id.as_ref(), next.thread_id.as_ref())
            {
                if req_tid <= held_tid {
                    return Err(LockError::ChildKeyOrder {
                        held: held_tid.clone(),
                        requested: req_tid.clone(),
                    });
                }
                return Ok(());
            }
            return Err(LockError::RankInversion {
                held: last.rank,
                requested: next.rank,
            });
        }
    }
    Ok(())
}
