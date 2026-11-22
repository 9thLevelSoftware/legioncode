//! Sequential FIFO [`DrainableWorker`] with deterministic `tokio::sync::Notify` draining.

use std::collections::VecDeque;
use std::fmt::Display;
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::Notify;
use tokio::task::{AbortHandle, JoinHandle};

fn lock_unpoison<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Outcome returned by a [`DrainableWorker`] task handler.
pub trait WorkerOutcome {
    /// Converts the task output into `true` on success or `false` on failure.
    fn into_succeeded(self) -> bool;
}

impl WorkerOutcome for () {
    #[inline]
    fn into_succeeded(self) -> bool {
        true
    }
}

impl<E: Display> WorkerOutcome for Result<(), E> {
    #[inline]
    fn into_succeeded(self) -> bool {
        self.is_ok()
    }
}

/// Point-in-time counters for a [`DrainableWorker`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerStats {
    /// Total queued plus currently running items.
    pub outstanding: usize,
    /// Items waiting in the FIFO queue.
    pub queued: usize,
    /// Number of items currently executing (`0` or `1`).
    pub in_flight: usize,
    /// Items completed successfully.
    pub completed: usize,
    /// Items that returned `Err` or panicked/cancelled.
    pub failed: usize,
    /// Whether the worker is still accepting new items.
    pub accepting: bool,
    /// Whether immediate close was triggered.
    pub closed: bool,
}

struct WorkerState<T> {
    queue: VecDeque<T>,
    outstanding: usize,
    accepting: bool,
    closed: bool,
    active_item: Option<AbortHandle>,
    completed: usize,
    failed: usize,
}

struct WorkerInner<T> {
    state: Mutex<WorkerState<T>>,
    work_available: Notify,
    drained: Notify,
}

struct LoopTaskGuard {
    handle: Mutex<Option<JoinHandle<()>>>,
}

impl Drop for LoopTaskGuard {
    fn drop(&mut self) {
        if let Some(handle) = lock_unpoison(&self.handle).take() {
            handle.abort();
        }
    }
}

/// Sequential background worker tracking queued + in-flight items with deterministic `drain()`.
///
/// Mirrors `packages/shared/src/DrainableWorker.ts`:
/// - Items execute strictly sequentially in FIFO enqueue order.
/// - Item errors or panics are isolated so subsequent queued items continue running.
/// - [`DrainableWorker::drain`] waits via [`tokio::sync::Notify`] with zero polling or sleeps,
///   including for items enqueued while another item is already in flight.
/// - [`DrainableWorker::close`] drops queued items, aborts any in-flight item, decrements the
///   outstanding counter, and unblocks pending `drain()` callers immediately.
///
/// # Examples
///
/// ```
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime").block_on(async {
/// use std::sync::{Arc, Mutex};
/// use legion_shared::concurrency::DrainableWorker;
///
/// let log = Arc::new(Mutex::new(Vec::new()));
/// let worker = DrainableWorker::new({
///     let log = Arc::clone(&log);
///     move |item: usize| {
///         let log = Arc::clone(&log);
///         async move {
///             log.lock().expect("lock").push(item);
///         }
///     }
/// });
///
/// assert!(worker.enqueue(1));
/// assert!(worker.enqueue(2));
/// worker.drain().await;
/// assert_eq!(*log.lock().expect("lock"), vec![1, 2]);
/// # });
/// ```
pub struct DrainableWorker<T> {
    inner: Arc<WorkerInner<T>>,
    loop_guard: Arc<LoopTaskGuard>,
}

impl<T> Clone for DrainableWorker<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            loop_guard: Arc::clone(&self.loop_guard),
        }
    }
}

impl<T> DrainableWorker<T>
where
    T: Send + 'static,
{
    /// Spawns a [`DrainableWorker`] that processes queued items sequentially using `handler`.
    ///
    /// Must be called inside a Tokio runtime context.
    pub fn new<F, Fut>(handler: F) -> Self
    where
        F: Fn(T) -> Fut + Send + Sync + 'static,
        Fut: Future + Send + 'static,
        Fut::Output: WorkerOutcome,
    {
        let inner = Arc::new(WorkerInner {
            state: Mutex::new(WorkerState {
                queue: VecDeque::new(),
                outstanding: 0,
                accepting: true,
                closed: false,
                active_item: None,
                completed: 0,
                failed: 0,
            }),
            work_available: Notify::new(),
            drained: Notify::new(),
        });

        let loop_handle = tokio::spawn(run_worker_loop(Arc::clone(&inner), Arc::new(handler)));
        Self {
            inner,
            loop_guard: Arc::new(LoopTaskGuard {
                handle: Mutex::new(Some(loop_handle)),
            }),
        }
    }

    /// Enqueues `item` for sequential processing.
    ///
    /// Returns `false` without queuing if [`DrainableWorker::close`] or
    /// [`DrainableWorker::shutdown`] has already been called.
    pub fn enqueue(&self, item: T) -> bool {
        {
            let mut state = lock_unpoison(&self.inner.state);
            if !state.accepting || state.closed {
                return false;
            }
            state.queue.push_back(item);
            state.outstanding += 1;
        }
        self.inner.work_available.notify_one();
        true
    }

    /// Waits deterministically until all queued and in-flight items have finished or been dropped.
    ///
    /// Uses [`tokio::sync::Notify`] with pre-registered enablement under the state mutex so no
    /// wakeup can be missed and zero polling or timer sleeps are used.
    pub async fn drain(&self) {
        loop {
            let mut notified = std::pin::pin!(self.inner.drained.notified());
            {
                let state = lock_unpoison(&self.inner.state);
                if state.outstanding == 0 {
                    return;
                }
                notified.as_mut().enable();
            }
            notified.await;
        }
    }

    /// Stops accepting new items while allowing already queued items to drain normally.
    pub async fn shutdown(&self) {
        {
            let mut state = lock_unpoison(&self.inner.state);
            state.accepting = false;
        }
        self.inner.work_available.notify_waiters();
        self.drain().await;
    }

    /// Immediately stops accepting items, drops unstarted queued items, aborts any in-flight
    /// item, and wakes any `drain()` waiters.
    pub fn close(&self) {
        let notify_drained = {
            let mut state = lock_unpoison(&self.inner.state);
            if state.closed {
                return;
            }
            state.accepting = false;
            state.closed = true;
            let dropped = state.queue.len();
            state.queue.clear();
            state.outstanding = state.outstanding.saturating_sub(dropped);
            if let Some(active) = state.active_item.take() {
                active.abort();
            }
            state.outstanding == 0
        };
        self.inner.work_available.notify_waiters();
        if notify_drained {
            self.inner.drained.notify_waiters();
        }
    }

    /// Returns a snapshot of current worker queue and execution counters.
    pub fn stats(&self) -> WorkerStats {
        let state = lock_unpoison(&self.inner.state);
        let queued = state.queue.len();
        WorkerStats {
            outstanding: state.outstanding,
            queued,
            in_flight: state.outstanding.saturating_sub(queued),
            completed: state.completed,
            failed: state.failed,
            accepting: state.accepting,
            closed: state.closed,
        }
    }
}

async fn run_worker_loop<T, F, Fut>(inner: Arc<WorkerInner<T>>, handler: Arc<F>)
where
    T: Send + 'static,
    F: Fn(T) -> Fut + Send + Sync + 'static,
    Fut: Future + Send + 'static,
    Fut::Output: WorkerOutcome,
{
    loop {
        let next_item = {
            let mut notified = std::pin::pin!(inner.work_available.notified());
            let popped = {
                let mut state = lock_unpoison(&inner.state);
                if state.closed {
                    return;
                }
                if let Some(item) = state.queue.pop_front() {
                    Some(item)
                } else if !state.accepting {
                    return;
                } else {
                    notified.as_mut().enable();
                    None
                }
            };
            let Some(item) = popped else {
                notified.await;
                continue;
            };
            item
        };

        let item_handler = Arc::clone(&handler);
        let handle = tokio::spawn(async move { item_handler(next_item).await.into_succeeded() });
        let abort_handle = handle.abort_handle();
        {
            let mut state = lock_unpoison(&inner.state);
            if state.closed {
                abort_handle.abort();
                state.outstanding = state.outstanding.saturating_sub(1);
                let drained = state.outstanding == 0;
                drop(state);
                if drained {
                    inner.drained.notify_waiters();
                }
                return;
            }
            state.active_item = Some(abort_handle);
        }

        let succeeded = matches!(handle.await, Ok(true));
        let notify_drained = {
            let mut state = lock_unpoison(&inner.state);
            state.active_item = None;
            state.outstanding = state.outstanding.saturating_sub(1);
            if succeeded {
                state.completed += 1;
            } else {
                state.failed += 1;
            }
            state.outstanding == 0
        };
        if notify_drained {
            inner.drained.notify_waiters();
        }
    }
}
