//! Per-key FIFO async mutex (`KeyedLock`) and ordered per-key task runner (`KeyedRunner`).

use std::collections::HashMap;
use std::future::Future;
use std::hash::Hash;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard, mpsc};

type BoxFuture = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

fn lock_unpoison<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

struct LockEntry {
    mutex: Arc<AsyncMutex<()>>,
    users: usize,
}

struct KeyedLockInner<K> {
    entries: Mutex<HashMap<K, LockEntry>>,
}

/// Cancellation-safe per-key FIFO mutex that evicts idle key entries on release.
///
/// Acquiring different keys proceeds concurrently; acquiring the same key serializes callers in
/// FIFO waiter order. When the last holder and all waiters for a key release or cancel, its map
/// entry is removed immediately.
///
/// Note: Not reentrant; locking the same key while already holding its [`KeyedLockGuard`] will
/// deadlock.
///
/// # Examples
///
/// ```
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime").block_on(async {
/// use t3_shared::concurrency::KeyedLock;
///
/// let lock = KeyedLock::new();
/// let value = lock.with_lock("thread-1", async { 42 }).await;
/// assert_eq!(value, 42);
/// assert!(lock.active_keys().is_empty());
/// # });
/// ```
pub struct KeyedLock<K> {
    inner: Arc<KeyedLockInner<K>>,
}

impl<K> Clone for KeyedLock<K> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<K> Default for KeyedLock<K>
where
    K: Eq + Hash + Clone,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<K> KeyedLock<K>
where
    K: Eq + Hash + Clone,
{
    /// Creates an empty [`KeyedLock`].
    pub fn new() -> Self {
        Self {
            inner: Arc::new(KeyedLockInner {
                entries: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// Acquires the exclusive FIFO lock for `key`, returning an RAII [`KeyedLockGuard`].
    ///
    /// If the future is dropped/cancelled before acquisition completes, the key reference count is
    /// still decremented and idle entries are cleaned up.
    pub async fn acquire(&self, key: K) -> KeyedLockGuard<K> {
        let ticket = self.checkout(key);
        let permit = Arc::clone(&ticket.mutex).lock_owned().await;
        KeyedLockGuard {
            _permit: permit,
            _ticket: ticket,
        }
    }

    /// Runs `fut` while holding the exclusive FIFO lock for `key`.
    pub async fn with_lock<F>(&self, key: K, fut: F) -> F::Output
    where
        F: Future,
    {
        let _guard = self.acquire(key).await;
        fut.await
    }

    /// Returns a snapshot of keys currently held or awaited.
    pub fn active_keys(&self) -> Vec<K> {
        lock_unpoison(&self.inner.entries).keys().cloned().collect()
    }

    /// Returns the number of keys currently held or awaited.
    pub fn active_key_count(&self) -> usize {
        lock_unpoison(&self.inner.entries).len()
    }

    fn checkout(&self, key: K) -> WaiterTicket<K> {
        let mutex = {
            let mut entries = lock_unpoison(&self.inner.entries);
            let entry = entries.entry(key.clone()).or_insert_with(|| LockEntry {
                mutex: Arc::new(AsyncMutex::new(())),
                users: 0,
            });
            entry.users += 1;
            Arc::clone(&entry.mutex)
        };
        WaiterTicket {
            inner: Arc::clone(&self.inner),
            key: Some(key),
            mutex,
        }
    }
}

struct WaiterTicket<K>
where
    K: Eq + Hash + Clone,
{
    inner: Arc<KeyedLockInner<K>>,
    key: Option<K>,
    mutex: Arc<AsyncMutex<()>>,
}

impl<K> Drop for WaiterTicket<K>
where
    K: Eq + Hash + Clone,
{
    fn drop(&mut self) {
        let Some(key) = self.key.take() else {
            return;
        };
        let mut entries = lock_unpoison(&self.inner.entries);
        let should_remove = if let Some(entry) = entries.get_mut(&key) {
            entry.users = entry.users.saturating_sub(1);
            entry.users == 0
        } else {
            false
        };
        if should_remove {
            entries.remove(&key);
        }
    }
}

/// RAII guard holding an exclusive [`KeyedLock`] lease for one key.
pub struct KeyedLockGuard<K>
where
    K: Eq + Hash + Clone,
{
    // Drop `_permit` BEFORE `_ticket` so the async mutex unlocks before the map entry is pruned.
    _permit: OwnedMutexGuard<()>,
    _ticket: WaiterTicket<K>,
}

/// Fire-and-forget per-key ordered task runner matching `KeyedLock.makeRunner` in `KeyedLock.ts`.
///
/// Each call to [`KeyedRunner::run`] synchronously enqueues its task onto `key`'s FIFO channel
/// before returning, guaranteeing strict call-order execution per key while distinct keys execute
/// concurrently.
pub struct KeyedRunner<K> {
    queues: Arc<Mutex<HashMap<K, mpsc::UnboundedSender<BoxFuture>>>>,
}

impl<K> Clone for KeyedRunner<K> {
    fn clone(&self) -> Self {
        Self {
            queues: Arc::clone(&self.queues),
        }
    }
}

impl<K> Default for KeyedRunner<K>
where
    K: Eq + Hash + Clone + Send + Sync + 'static,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<K> KeyedRunner<K>
where
    K: Eq + Hash + Clone + Send + Sync + 'static,
{
    /// Creates a new [`KeyedRunner`].
    pub fn new() -> Self {
        Self {
            queues: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Synchronously queues `fut` to execute after all previously queued tasks for `key`.
    ///
    /// Must be called inside a Tokio runtime context.
    pub fn run<F>(&self, key: K, fut: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let boxed: BoxFuture = Box::pin(fut);
        let mut queues = lock_unpoison(&self.queues);

        let boxed = match queues.get(&key) {
            Some(existing) => match existing.send(boxed) {
                Ok(()) => return,
                Err(err) => err.0,
            },
            None => boxed,
        };

        let (tx, mut rx) = mpsc::unbounded_channel();
        if tx.send(boxed).is_err() {
            // `rx` is still alive locally so `send` cannot fail; early return keeps code total.
            return;
        }

        queues.insert(key.clone(), tx.clone());
        drop(queues);

        let queues_ref = Arc::clone(&self.queues);
        tokio::spawn(async move {
            loop {
                let next = match rx.try_recv() {
                    Ok(task) => Some(task),
                    Err(mpsc::error::TryRecvError::Empty) => {
                        let mut guard = lock_unpoison(&queues_ref);
                        match rx.try_recv() {
                            Ok(task) => Some(task),
                            Err(
                                mpsc::error::TryRecvError::Empty
                                | mpsc::error::TryRecvError::Disconnected,
                            ) => {
                                if guard.get(&key).is_some_and(|cur| cur.same_channel(&tx)) {
                                    guard.remove(&key);
                                }
                                None
                            }
                        }
                    }
                    Err(mpsc::error::TryRecvError::Disconnected) => None,
                };
                match next {
                    Some(task) => task.await,
                    None => break,
                }
            }
        });
    }

    /// Returns keys that currently have an active runner queue.
    pub fn active_keys(&self) -> Vec<K> {
        lock_unpoison(&self.queues).keys().cloned().collect()
    }
}
