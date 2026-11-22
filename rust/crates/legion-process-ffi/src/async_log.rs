//! Non-blocking bounded channel log writer (`90c028c6fc` #39857).
//!
//! Ensures slow or stalled filesystem appends never freeze server or turn worker threads:
//! callers enqueue via `try_send` against a bounded channel and byte cap, falling back to
//! atomic dropped-line / dropped-byte counters whenever backpressure saturates.

use std::io::{self, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::thread::{self, JoinHandle};

use thiserror::Error;

/// Default bounded message queue depth for [`AsyncLogWriter`].
pub const DEFAULT_LOG_CHANNEL_CAPACITY: usize = 1024;
/// Default maximum pending/in-flight byte budget (`8 MiB`, matching `serviceLauncher.ts:62`).
pub const DEFAULT_MAX_PENDING_LOG_BYTES: usize = 8 * 1024 * 1024;

/// Configuration for [`AsyncLogWriter`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AsyncLogConfig {
    /// Maximum queued log entries held in the bounded `mpsc::sync_channel`.
    pub channel_capacity: usize,
    /// Maximum cumulative queued + in-flight bytes before dropping new log output.
    pub max_pending_bytes: usize,
}

impl Default for AsyncLogConfig {
    fn default() -> Self {
        Self {
            channel_capacity: DEFAULT_LOG_CHANNEL_CAPACITY,
            max_pending_bytes: DEFAULT_MAX_PENDING_LOG_BYTES,
        }
    }
}

/// Outcome returned by a non-blocking write call on [`AsyncLogWriter`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogEnqueueStatus {
    /// Entry was accepted by the bounded channel without blocking.
    Queued,
    /// Dropped because `max_pending_bytes` was exceeded while disk I/O lagged.
    DroppedBackpressure,
    /// Dropped because the bounded `sync_channel` was full.
    DroppedFull,
    /// Dropped because the writer thread has already shut down.
    Closed,
}

/// Point-in-time metrics snapshot from [`AsyncLogWriter::stats`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AsyncLogStats {
    pub pending_bytes: usize,
    pub dropped_lines: u64,
    pub dropped_bytes: u64,
    pub written_lines: u64,
    pub written_bytes: u64,
    pub write_errors: u64,
}

/// Errors returned when constructing or flushing an [`AsyncLogWriter`].
#[derive(Debug, Error)]
pub enum AsyncLogError {
    #[error("async log channel_capacity and max_pending_bytes must be non-zero")]
    InvalidConfig,
    #[error("failed to spawn async log writer thread: {0}")]
    ThreadSpawn(#[source] io::Error),
    #[error("async log writer thread is no longer running")]
    Closed,
}

enum LogMessage {
    Write(Vec<u8>),
    Flush(SyncSender<()>),
}

#[derive(Debug, Default)]
struct AsyncLogCounters {
    pending_bytes: AtomicUsize,
    dropped_lines: AtomicU64,
    dropped_bytes: AtomicU64,
    written_lines: AtomicU64,
    written_bytes: AtomicU64,
    write_errors: AtomicU64,
}

impl AsyncLogCounters {
    fn snapshot(&self) -> AsyncLogStats {
        AsyncLogStats {
            pending_bytes: self.pending_bytes.load(Ordering::Relaxed),
            dropped_lines: self.dropped_lines.load(Ordering::Relaxed),
            dropped_bytes: self.dropped_bytes.load(Ordering::Relaxed),
            written_lines: self.written_lines.load(Ordering::Relaxed),
            written_bytes: self.written_bytes.load(Ordering::Relaxed),
            write_errors: self.write_errors.load(Ordering::Relaxed),
        }
    }

    fn try_reserve_bytes(&self, len: usize, limit: usize) -> bool {
        let mut current = self.pending_bytes.load(Ordering::Relaxed);
        loop {
            let Some(next) = current.checked_add(len) else {
                return false;
            };
            if next > limit {
                return false;
            }
            match self.pending_bytes.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(observed) => current = observed,
            }
        }
    }

    fn release_bytes(&self, len: usize) {
        let mut current = self.pending_bytes.load(Ordering::Relaxed);
        loop {
            let next = current.saturating_sub(len);
            match self.pending_bytes.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => return,
                Err(observed) => current = observed,
            }
        }
    }

    fn record_drop(&self, len: usize) {
        self.dropped_lines.fetch_add(1, Ordering::Relaxed);
        self.dropped_bytes
            .fetch_add(u64::try_from(len).unwrap_or(u64::MAX), Ordering::Relaxed);
    }
}

/// Non-blocking bounded-channel log sink backed by a dedicated background writer thread.
///
/// Dropping [`AsyncLogWriter`] closes the channel without blocking on a stalled sink; call
/// [`Self::shutdown`] to wait for queued writes to drain and join the background thread.
#[derive(Debug)]
pub struct AsyncLogWriter {
    tx: Option<SyncSender<LogMessage>>,
    config: AsyncLogConfig,
    counters: Arc<AsyncLogCounters>,
    worker: Option<JoinHandle<()>>,
}

impl AsyncLogWriter {
    /// Spawns a background writer draining bounded entries into `sink`.
    pub fn spawn<W: Write + Send + 'static>(
        sink: W,
        config: AsyncLogConfig,
    ) -> Result<Self, AsyncLogError> {
        if config.channel_capacity == 0 || config.max_pending_bytes == 0 {
            return Err(AsyncLogError::InvalidConfig);
        }
        let (tx, rx) = mpsc::sync_channel(config.channel_capacity);
        let counters = Arc::new(AsyncLogCounters::default());
        let worker_counters = Arc::clone(&counters);
        let worker = thread::Builder::new()
            .name("legion-async-log".to_owned())
            .spawn(move || run_writer_loop(sink, &rx, &worker_counters))
            .map_err(AsyncLogError::ThreadSpawn)?;
        Ok(Self {
            tx: Some(tx),
            config,
            counters,
            worker: Some(worker),
        })
    }

    /// Enqueues a log line without blocking, appending `'\n'` if missing.
    pub fn try_write_line(&self, line: &str) -> LogEnqueueStatus {
        let mut buf = Vec::with_capacity(line.len() + usize::from(!line.ends_with('\n')));
        buf.extend_from_slice(line.as_bytes());
        if !line.ends_with('\n') {
            buf.push(b'\n');
        }
        self.try_send_owned(buf)
    }

    /// Enqueues raw log bytes without blocking, dropping with counter update on backpressure.
    pub fn try_write_bytes(&self, bytes: &[u8]) -> LogEnqueueStatus {
        if bytes.is_empty() {
            return LogEnqueueStatus::Queued;
        }
        self.try_send_owned(bytes.to_vec())
    }

    fn try_send_owned(&self, buf: Vec<u8>) -> LogEnqueueStatus {
        let Some(tx) = self.tx.as_ref() else {
            self.counters.record_drop(buf.len());
            return LogEnqueueStatus::Closed;
        };
        let len = buf.len();
        if !self
            .counters
            .try_reserve_bytes(len, self.config.max_pending_bytes)
        {
            self.counters.record_drop(len);
            return LogEnqueueStatus::DroppedBackpressure;
        }
        match tx.try_send(LogMessage::Write(buf)) {
            Ok(()) => LogEnqueueStatus::Queued,
            Err(TrySendError::Full(_)) => {
                self.counters.release_bytes(len);
                self.counters.record_drop(len);
                LogEnqueueStatus::DroppedFull
            }
            Err(TrySendError::Disconnected(_)) => {
                self.counters.release_bytes(len);
                self.counters.record_drop(len);
                LogEnqueueStatus::Closed
            }
        }
    }

    /// Waits for currently queued log entries and the underlying sink to flush.
    pub fn flush(&self) -> Result<(), AsyncLogError> {
        let Some(tx) = self.tx.as_ref() else {
            return Err(AsyncLogError::Closed);
        };
        let (ack_tx, ack_rx) = mpsc::sync_channel(1);
        tx.send(LogMessage::Flush(ack_tx))
            .map_err(|_| AsyncLogError::Closed)?;
        ack_rx.recv().map_err(|_| AsyncLogError::Closed)
    }

    /// Returns the total number of log lines/chunks dropped due to backpressure or full queue.
    pub fn dropped_lines(&self) -> u64 {
        self.counters.dropped_lines.load(Ordering::Relaxed)
    }

    /// Returns the current [`AsyncLogStats`] metrics snapshot.
    pub fn stats(&self) -> AsyncLogStats {
        self.counters.snapshot()
    }

    /// Closes the channel, waits for queued writes to drain, and joins the background thread.
    pub fn shutdown(mut self) {
        self.tx.take();
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for AsyncLogWriter {
    fn drop(&mut self) {
        // Disconnect without joining so dropping on a stalled disk never stalls the caller thread.
        self.tx.take();
    }
}

fn run_writer_loop<W: Write>(mut sink: W, rx: &Receiver<LogMessage>, counters: &AsyncLogCounters) {
    while let Ok(msg) = rx.recv() {
        match msg {
            LogMessage::Write(bytes) => write_best_effort(&mut sink, &bytes, counters),
            LogMessage::Flush(ack) => {
                if sink.flush().is_err() {
                    counters.write_errors.fetch_add(1, Ordering::Relaxed);
                }
                let _ = ack.try_send(());
            }
        }
    }
    let _ = sink.flush();
}

fn write_best_effort<W: Write>(sink: &mut W, bytes: &[u8], counters: &AsyncLogCounters) {
    let len = bytes.len();
    if sink.write_all(bytes).is_ok() {
        counters.written_lines.fetch_add(1, Ordering::Relaxed);
        counters
            .written_bytes
            .fetch_add(u64::try_from(len).unwrap_or(u64::MAX), Ordering::Relaxed);
    } else {
        counters.write_errors.fetch_add(1, Ordering::Relaxed);
    }
    counters.release_bytes(len);
}
