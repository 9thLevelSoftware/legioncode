#![cfg(feature = "concurrency")]
//! Concurrency property and determinism tests for [`legion_shared::concurrency`].

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::{Mutex, Notify, oneshot};

use legion_shared::concurrency::{DrainableWorker, KeyedLock, KeyedRunner};

#[tokio::test]
async fn keyed_lock_cleanup_on_cancelled_waiter_and_concurrent_keys() {
    let lock = KeyedLock::new();
    let entered = Arc::new(Notify::new());
    let (release_tx, release_rx) = oneshot::channel::<()>();

    let holder = tokio::spawn({
        let lock = lock.clone();
        let entered = Arc::clone(&entered);
        async move {
            lock.with_lock("k", async {
                entered.notify_one();
                let _ = release_rx.await;
            })
            .await;
        }
    });
    entered.notified().await;

    let abandoned = tokio::spawn({
        let lock = lock.clone();
        async move {
            lock.with_lock("k", async {}).await;
        }
    });
    tokio::task::yield_now().await;
    assert_eq!(lock.active_key_count(), 1);

    abandoned.abort();
    let _ = abandoned.await;
    assert_eq!(lock.active_key_count(), 1);

    let _ = release_tx.send(());
    let _ = holder.await;
    assert_eq!(lock.active_keys(), Vec::<&str>::new());

    // Distinct keys execute concurrently without blocking one another.
    let both_entered = Arc::new(AtomicUsize::new(0));
    let both_ready = Arc::new(Notify::new());
    let release_both = Arc::new(Notify::new());
    let spawn_key = |key: &'static str| {
        tokio::spawn({
            let lock = lock.clone();
            let both_entered = Arc::clone(&both_entered);
            let both_ready = Arc::clone(&both_ready);
            let release_both = Arc::clone(&release_both);
            async move {
                lock.with_lock(key, async {
                    if both_entered.fetch_add(1, Ordering::SeqCst) + 1 == 2 {
                        both_ready.notify_one();
                    }
                    release_both.notified().await;
                })
                .await;
            }
        })
    };
    let h1 = spawn_key("a");
    let h2 = spawn_key("b");
    both_ready.notified().await;
    release_both.notify_waiters();
    let _ = tokio::join!(h1, h2);
    assert_eq!(lock.active_keys(), Vec::<&str>::new());
}

#[tokio::test]
async fn keyed_runner_keeps_per_key_fifo_and_isolates_other_keys() {
    let runner = KeyedRunner::new();
    let events = Arc::new(Mutex::new(Vec::<&'static str>::new()));
    let (release_tx, release_rx) = oneshot::channel::<()>();
    let other_done = Arc::new(Notify::new());
    let busy_done = Arc::new(Notify::new());

    runner.run("busy", {
        let events = Arc::clone(&events);
        async move {
            let _ = release_rx.await;
            events.lock().await.push("busy:1");
        }
    });
    runner.run("busy", {
        let events = Arc::clone(&events);
        async move {
            events.lock().await.push("busy:2");
        }
    });
    runner.run("busy", {
        let events = Arc::clone(&events);
        let busy_done = Arc::clone(&busy_done);
        async move {
            events.lock().await.push("busy:3");
            busy_done.notify_one();
        }
    });
    runner.run("other", {
        let events = Arc::clone(&events);
        let other_done = Arc::clone(&other_done);
        async move {
            events.lock().await.push("other");
            other_done.notify_one();
        }
    });

    other_done.notified().await;
    assert_eq!(*events.lock().await, vec!["other"]);

    let _ = release_tx.send(());
    busy_done.notified().await;
    assert_eq!(
        *events.lock().await,
        vec!["other", "busy:1", "busy:2", "busy:3"]
    );
}

#[tokio::test]
async fn drainable_worker_isolates_panics_errors_and_handles_close() {
    let processed = Arc::new(Mutex::new(Vec::<&'static str>::new()));
    let worker = DrainableWorker::new({
        let processed = Arc::clone(&processed);
        move |item: &'static str| {
            let processed = Arc::clone(&processed);
            async move {
                if item == "err" {
                    return Err("typed failure");
                }
                assert!(item != "panic", "simulated defect");
                processed.lock().await.push(item);
                Ok(())
            }
        }
    });

    assert!(worker.enqueue("err"));
    assert!(worker.enqueue("panic"));
    assert!(worker.enqueue("ok"));
    worker.drain().await;

    assert_eq!(*processed.lock().await, vec!["ok"]);
    let stats = worker.stats();
    assert_eq!(stats.completed, 1);
    assert_eq!(stats.failed, 2);
    assert_eq!(stats.outstanding, 0);

    // Closing mid-flight drops queued items and resolves drain() immediately.
    let started = Arc::new(Notify::new());
    let blocker = DrainableWorker::new({
        let started = Arc::clone(&started);
        move |_item: &'static str| {
            let started = Arc::clone(&started);
            async move {
                started.notify_one();
                std::future::pending::<()>().await;
            }
        }
    });
    assert!(blocker.enqueue("block"));
    assert!(blocker.enqueue("dropped"));
    started.notified().await;
    blocker.close();
    blocker.drain().await;
    assert!(!blocker.enqueue("after-close"));
    assert_eq!(blocker.stats().outstanding, 0);
}
