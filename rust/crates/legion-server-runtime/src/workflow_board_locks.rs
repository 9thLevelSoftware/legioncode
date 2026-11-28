#![forbid(unsafe_code)]
//! Per-card async single-flight lock registry for Project Workflow Board mutations.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex, OnceLock, Weak};

use legion_orchestrator::Orchestrator;
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

type CardLockMap = HashMap<(usize, String), Weak<AsyncMutex<()>>>;

fn card_lock_registry() -> &'static StdMutex<CardLockMap> {
    static REGISTRY: OnceLock<StdMutex<CardLockMap>> = OnceLock::new();
    REGISTRY.get_or_init(|| StdMutex::new(HashMap::new()))
}

pub(crate) struct CardLockGuard {
    key: (usize, String),
    guard: Option<OwnedMutexGuard<()>>,
    mutex: Option<Arc<AsyncMutex<()>>>,
}

impl Drop for CardLockGuard {
    fn drop(&mut self) {
        drop(self.guard.take());
        drop(self.mutex.take());
        let mut map = card_lock_registry()
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

pub(crate) async fn acquire_card_lock(
    orchestrator: &Arc<Orchestrator>,
    card_id: &str,
) -> CardLockGuard {
    let key = (Arc::as_ptr(orchestrator) as usize, card_id.to_owned());
    let mutex = {
        let mut map = card_lock_registry()
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
    CardLockGuard {
        key,
        guard: Some(guard),
        mutex: Some(mutex),
    }
}
