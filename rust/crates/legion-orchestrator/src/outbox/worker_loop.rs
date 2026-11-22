//! Background worker execution loop and retry settlement for [`crate::outbox::EffectOutboxWorker`].

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, PoisonError};

use rusqlite::Connection;

use crate::outbox::store;
use crate::outbox::types::{OutboxEffect, OutboxStoreError};
use crate::outbox::worker_types::{OutboxEffectExecutor, OutboxSignal, OutboxWorkerStats};
use crate::time::now_iso_utc;

#[derive(Debug)]
pub(crate) struct CoordState {
    pub(crate) in_flight: usize,
    pub(crate) completed: usize,
    pub(crate) failed_attempts: usize,
    pub(crate) accepting: bool,
    pub(crate) closed: bool,
}

pub(crate) struct WorkerInner {
    pub(crate) db: Arc<Mutex<Connection>>,
    pub(crate) executor: Arc<dyn OutboxEffectExecutor>,
    pub(crate) worker_id: String,
    pub(crate) max_attempts: u64,
    pub(crate) retry_delay_ms: i64,
    pub(crate) signal: Arc<OutboxSignal>,
    pub(crate) coord: Mutex<CoordState>,
}

pub(crate) fn stats_from(st: &CoordState) -> OutboxWorkerStats {
    OutboxWorkerStats {
        in_flight: st.in_flight,
        completed: st.completed,
        failed_attempts: st.failed_attempts,
        closed: st.closed,
    }
}

pub(crate) fn lock_unpoison<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(crate) async fn worker_loop(inner: Arc<WorkerInner>) {
    loop {
        let mut work_notified = std::pin::pin!(inner.signal.work_available.notified());
        let gen_before = inner.signal.generation.load(Ordering::SeqCst);
        let claimed = {
            let mut st = lock_unpoison(&inner.coord);
            if st.closed {
                inner.signal.drained.notify_waiters();
                return;
            }
            let claim = store::with_locked_conn(&inner.db, |conn| {
                store::claim_next_effect(conn, &inner.worker_id, &now_iso_utc())
            });
            match claim {
                Ok(Some(effect)) => {
                    st.in_flight += 1;
                    Some(effect)
                }
                Ok(None) => {
                    if st.in_flight == 0 {
                        inner.signal.drained.notify_waiters();
                    }
                    if !st.accepting {
                        return;
                    }
                    work_notified.as_mut().enable();
                    None
                }
                Err(_) => {
                    if st.in_flight == 0 {
                        inner.signal.drained.notify_waiters();
                    }
                    work_notified.as_mut().enable();
                    None
                }
            }
        };

        let Some(effect) = claimed else {
            if inner.signal.generation.load(Ordering::SeqCst) != gen_before {
                continue;
            }
            work_notified.await;
            continue;
        };

        let _ = execute_and_settle(&inner, effect).await;
        let mut st = lock_unpoison(&inner.coord);
        st.in_flight = st.in_flight.saturating_sub(1);
    }
}

pub(crate) async fn execute_and_settle(
    inner: &WorkerInner,
    effect: OutboxEffect,
) -> Result<(), OutboxStoreError> {
    let will_retry = effect.attempt_count < inner.max_attempts;
    let effect_id = effect.id.clone();
    let res = inner.executor.execute(effect, will_retry).await;
    let now_iso = now_iso_utc();
    match res {
        Ok(()) => {
            store::with_locked_conn(&inner.db, |conn| {
                store::mark_succeeded(conn, &effect_id, &inner.worker_id, &now_iso)
            })?;
            lock_unpoison(&inner.coord).completed += 1;
        }
        Err(err) => {
            store::with_locked_conn(&inner.db, |conn| {
                if will_retry {
                    store::mark_retry(
                        conn,
                        &effect_id,
                        &inner.worker_id,
                        &err,
                        inner.retry_delay_ms,
                        &now_iso,
                    )
                } else {
                    store::mark_failed(conn, &effect_id, &inner.worker_id, &err, &now_iso)
                }
            })?;
            lock_unpoison(&inner.coord).failed_attempts += 1;
        }
    }
    Ok(())
}
