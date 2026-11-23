//! Live terminal session manager wiring `TerminalSessionKeys` (`BUG-08`), `VtParser`, `AnsiPalette256` (`BUG-25`), `"LCVT"` snapshots, interactive OS PTYs, and broadcast streams.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};

use legion_protocol::provider_env::{
    TerminalResumeStatus, TerminalSessionKey, TerminalSessionKeys,
};
use legion_terminal_vt::{Rgba, encode_lcvt};
use tokio::sync::{Notify, broadcast};

use crate::error::ServerRuntimeError;
use crate::terminal_session::{
    DEFAULT_STREAM_CHANNEL_CAPACITY, EvictedSessionMeta, TerminalMetadataStreamEvent,
    TerminalOpenRequest, TerminalOpenResult, TerminalOutputSubscription, TerminalSessionEntry,
    TerminalSessionStateSnapshot, TerminalSessionSummary, TerminalSnapshotResult, active_entry,
    active_entry_mut, build_opened_entry, collect_summaries, prepare_write_bytes,
    validate_resume_eligibility,
};

struct RuntimeState {
    keys: TerminalSessionKeys,
    active: HashMap<TerminalSessionKey, TerminalSessionEntry>,
    evicted_meta: HashMap<TerminalSessionKey, EvictedSessionMeta>,
    metadata_tx: broadcast::Sender<TerminalMetadataStreamEvent>,
}

/// Thread-safe live terminal runtime managing VT grids, palettes, snapshots, and child PTY/pipe processes.
#[derive(Clone)]
pub struct TerminalRuntime {
    state: Arc<StdMutex<RuntimeState>>,
}

impl Default for TerminalRuntime {
    fn default() -> Self {
        Self::with_capacity(32)
    }
}

impl TerminalRuntime {
    /// Create a [`TerminalRuntime`] retaining up to `max_inactive_retained` inactive sessions before LRU eviction (`BUG-08`).
    pub fn with_capacity(max_inactive_retained: usize) -> Self {
        let (metadata_tx, _) = broadcast::channel(DEFAULT_STREAM_CHANNEL_CAPACITY);
        Self {
            state: Arc::new(StdMutex::new(RuntimeState {
                keys: TerminalSessionKeys::with_capacity(max_inactive_retained),
                active: HashMap::new(),
                evicted_meta: HashMap::new(),
                metadata_tx,
            })),
        }
    }

    /// Open a new terminal session or resume a retained/evicted session (`BUG-08` forbids resuming explicitly closed sessions).
    pub fn terminal_open(
        &self,
        req: &TerminalOpenRequest,
    ) -> Result<TerminalOpenResult, ServerRuntimeError> {
        let key = TerminalSessionKey::new(&req.thread_id, &req.terminal_id);
        let mut guard = lock_state(&self.state)?;
        validate_resume_eligibility(&guard.keys, &key, req.resume)?;

        let was_evicted = guard.keys.is_evicted(&key);
        if req.resume
            && let Some(existing) = guard.active.get(&key)
        {
            let (cols, rows) = existing
                .grid
                .lock()
                .map_or((req.cols, req.rows), |g| (g.cols(), g.rows()));
            guard.keys.open(key.clone());
            let status = guard.keys.status(&key);
            return Ok(TerminalOpenResult {
                key,
                resumed: true,
                was_evicted: false,
                cols,
                rows,
                status,
            });
        }

        let meta = if req.resume {
            guard.evicted_meta.remove(&key)
        } else {
            let _ = guard.evicted_meta.remove(&key);
            None
        };
        let metadata_tx = guard.metadata_tx.clone();
        let (cols, rows, entry) = build_opened_entry(req, meta, metadata_tx)?;

        if let Some(mut old) = guard.active.remove(&key)
            && let Some(mut proc_handle) = old.process.take()
        {
            proc_handle.terminate();
        }
        guard.active.insert(key.clone(), entry);
        guard.keys.open(key.clone());
        let status = guard.keys.status(&key);

        Ok(TerminalOpenResult {
            key,
            resumed: req.resume,
            was_evicted,
            cols,
            rows,
            status,
        })
    }

    /// Mark an active session inactive; if retention capacity is exceeded, evicts oldest inactive session(s) (`BUG-08`).
    pub fn mark_inactive(
        &self,
        thread_id: &str,
        terminal_id: &str,
    ) -> Result<Vec<TerminalSessionKey>, ServerRuntimeError> {
        let key = TerminalSessionKey::new(thread_id, terminal_id);
        let mut guard = lock_state(&self.state)?;
        let evicted = guard.keys.mark_inactive(key);
        for ev_key in &evicted {
            if let Some(mut entry) = guard.active.remove(ev_key) {
                if let Some(mut proc_handle) = entry.process.take() {
                    proc_handle.terminate();
                }
                if let Ok(mut s) = entry.stream.lock() {
                    s.mark_closed();
                }
                let (cols, rows) = entry.grid.lock().map_or((80, 24), |g| (g.cols(), g.rows()));
                guard.evicted_meta.insert(
                    ev_key.clone(),
                    EvictedSessionMeta {
                        cols,
                        rows,
                        cwd: entry.cwd,
                        worktree_path: entry.worktree_path,
                        palette_overrides: entry.palette_overrides,
                    },
                );
            }
        }
        Ok(evicted)
    }

    /// Write input or sanitized paste text to the session process or VT parser.
    pub fn terminal_write(
        &self,
        thread_id: &str,
        terminal_id: &str,
        data: &str,
        as_paste: bool,
    ) -> Result<(), ServerRuntimeError> {
        let key = TerminalSessionKey::new(thread_id, terminal_id);
        let mut guard = lock_state(&self.state)?;
        let entry = active_entry_mut(&mut guard.active, &key)?;
        let bracketed = entry.grid.lock().is_ok_and(|g| g.bracketed_paste_enabled());
        let bytes = prepare_write_bytes(data, as_paste, bracketed);
        if let Some(proc_handle) = entry.process.as_mut() {
            proc_handle.write_bytes(&bytes)?;
        } else {
            let title = if let (Ok(mut g), Ok(mut p)) = (entry.grid.lock(), entry.parser.lock()) {
                p.feed(&bytes, &mut g);
                let t = g.title();
                (!t.is_empty()).then(|| t.to_owned())
            } else {
                None
            };
            if let Ok(mut s) = entry.stream.lock() {
                s.push_raw_output(&bytes, title.as_deref());
            }
            entry.output_notify.notify_waiters();
        }
        Ok(())
    }

    /// Resize the session's `TerminalGrid` and any attached OS PTY (`PtyProcess::resize`).
    pub fn terminal_resize(
        &self,
        thread_id: &str,
        terminal_id: &str,
        cols: u16,
        rows: u16,
    ) -> Result<(), ServerRuntimeError> {
        if cols == 0 || rows == 0 {
            return Err(ServerRuntimeError::from(
                legion_terminal_vt::VtError::InvalidDimensions { cols, rows },
            ));
        }
        let key = TerminalSessionKey::new(thread_id, terminal_id);
        let guard = lock_state(&self.state)?;
        let entry = active_entry(&guard.active, &key)?;
        if let Some(proc_handle) = entry.process.as_ref() {
            proc_handle.resize(cols, rows)?;
        }
        let mut g = entry
            .grid
            .lock()
            .map_err(|_| ServerRuntimeError::Terminal("grid mutex poisoned".to_owned()))?;
        g.resize(cols, rows);
        drop(g);
        if let Ok(mut s) = entry.stream.lock() {
            s.mark_resize(cols, rows);
        }
        Ok(())
    }

    /// Clear the active session's viewport, scrollback buffer, and broadcast history replay cache.
    pub fn terminal_clear(
        &self,
        thread_id: &str,
        terminal_id: &str,
    ) -> Result<(), ServerRuntimeError> {
        let key = TerminalSessionKey::new(thread_id, terminal_id);
        let guard = lock_state(&self.state)?;
        let entry = active_entry(&guard.active, &key)?;
        if let (Ok(mut g), Ok(mut p)) = (entry.grid.lock(), entry.parser.lock()) {
            p.feed(b"\x1b[2J\x1b[3J\x1b[H", &mut g);
        }
        if let Ok(mut s) = entry.stream.lock() {
            s.mark_cleared();
        }
        entry.output_notify.notify_waiters();
        Ok(())
    }

    /// Subscribe atomically to an active session's output/lifecycle broadcast stream + initial history snapshot.
    pub fn subscribe_output(
        &self,
        thread_id: &str,
        terminal_id: &str,
    ) -> Result<TerminalOutputSubscription, ServerRuntimeError> {
        self.subscribe_output_with_limit(thread_id, terminal_id, None)
    }

    /// Subscribe atomically with an optional `max_history_bytes` replay cap (matching `TerminalAttachInput.maxHistoryBytes`).
    pub fn subscribe_output_with_limit(
        &self,
        thread_id: &str,
        terminal_id: &str,
        max_history_bytes: Option<usize>,
    ) -> Result<TerminalOutputSubscription, ServerRuntimeError> {
        let key = TerminalSessionKey::new(thread_id, terminal_id);
        let guard = lock_state(&self.state)?;
        let entry = active_entry(&guard.active, &key)?;
        let stream = entry
            .stream
            .lock()
            .map_err(|_| ServerRuntimeError::Terminal("stream mutex poisoned".to_owned()))?;
        Ok(stream.subscribe(max_history_bytes))
    }

    /// Fetch the wire-compatible session state snapshot including sanitized scrollback history.
    pub fn session_snapshot(
        &self,
        thread_id: &str,
        terminal_id: &str,
        max_history_bytes: Option<usize>,
    ) -> Result<TerminalSessionStateSnapshot, ServerRuntimeError> {
        let key = TerminalSessionKey::new(thread_id, terminal_id);
        let guard = lock_state(&self.state)?;
        let entry = active_entry(&guard.active, &key)?;
        let stream = entry
            .stream
            .lock()
            .map_err(|_| ServerRuntimeError::Terminal("stream mutex poisoned".to_owned()))?;
        Ok(stream.snapshot(max_history_bytes))
    }

    /// List lightweight metadata summaries for active sessions (optionally filtered to one `thread_id`).
    pub fn list_summaries(&self, thread_id: Option<&str>) -> Vec<TerminalSessionSummary> {
        let Ok(guard) = self.state.lock() else {
            return Vec::new();
        };
        collect_summaries(&guard.active, thread_id)
    }

    /// Atomically captures the current fleet metadata snapshot and subscribes to future upserts/removals.
    pub fn subscribe_metadata(
        &self,
    ) -> Result<
        (
            TerminalMetadataStreamEvent,
            broadcast::Receiver<TerminalMetadataStreamEvent>,
        ),
        ServerRuntimeError,
    > {
        let guard = lock_state(&self.state)?;
        let receiver = guard.metadata_tx.subscribe();
        let terminals = collect_summaries(&guard.active, None);
        Ok((
            TerminalMetadataStreamEvent::Snapshot { terminals },
            receiver,
        ))
    }

    /// Apply 256-color palette overrides (`BUG-25`) to an active terminal's `AnsiPalette256`.
    pub fn apply_palette_overrides(
        &self,
        thread_id: &str,
        terminal_id: &str,
        overrides: &[(u8, Rgba)],
    ) -> Result<(), ServerRuntimeError> {
        let key = TerminalSessionKey::new(thread_id, terminal_id);
        let mut guard = lock_state(&self.state)?;
        let entry = active_entry_mut(&mut guard.active, &key)?;
        entry.palette_overrides.extend_from_slice(overrides);
        let mut g = entry
            .grid
            .lock()
            .map_err(|_| ServerRuntimeError::Terminal("grid mutex poisoned".to_owned()))?;
        g.palette_mut().apply_theme_overrides(overrides);
        Ok(())
    }

    /// Encode the session's current viewport and palette into a `"LCVT"` binary snapshot.
    pub fn terminal_snapshot(
        &self,
        thread_id: &str,
        terminal_id: &str,
    ) -> Result<TerminalSnapshotResult, ServerRuntimeError> {
        let key = TerminalSessionKey::new(thread_id, terminal_id);
        let guard = lock_state(&self.state)?;
        let entry = active_entry(&guard.active, &key)?;
        let g = entry
            .grid
            .lock()
            .map_err(|_| ServerRuntimeError::Terminal("grid mutex poisoned".to_owned()))?;
        let snap = g.snapshot();
        let lcvt_bytes = encode_lcvt(&snap);
        Ok(TerminalSnapshotResult {
            lcvt_bytes,
            cols: g.cols(),
            rows: g.rows(),
            cursor_x: g.cursor_x(),
            cursor_y: g.cursor_y(),
            viewport_text: g.viewport_text(),
        })
    }

    /// Explicitly close a terminal session (`BUG-08`: permanently prevents `resume: true`).
    pub fn terminal_close(
        &self,
        thread_id: &str,
        terminal_id: &str,
    ) -> Result<(), ServerRuntimeError> {
        let key = TerminalSessionKey::new(thread_id, terminal_id);
        let mut guard = lock_state(&self.state)?;
        if let Some(mut entry) = guard.active.remove(&key) {
            if let Some(mut proc_handle) = entry.process.take() {
                proc_handle.terminate();
            }
            if let Ok(mut s) = entry.stream.lock() {
                s.mark_closed();
            }
        }
        let _ = guard.evicted_meta.remove(&key);
        guard.keys.close(key);
        Ok(())
    }

    /// Query resume lifecycle status (`BUG-08`) and `(active, inactive, evicted, closed)` counts.
    pub fn resume_status(&self, thread_id: &str, terminal_id: &str) -> TerminalResumeStatus {
        let key = TerminalSessionKey::new(thread_id, terminal_id);
        self.state
            .lock()
            .map_or(TerminalResumeStatus::Unknown, |g| g.keys.status(&key))
    }

    /// Return `(active, inactive, evicted, closed)` session counts tracked by [`TerminalSessionKeys`].
    pub fn counts(&self) -> (usize, usize, usize, usize) {
        self.state.lock().map_or((0, 0, 0, 0), |g| g.keys.counts())
    }

    /// Return the notification handle fired when an active session receives new VT bytes.
    pub fn output_notifier(&self, thread_id: &str, terminal_id: &str) -> Option<Arc<Notify>> {
        let key = TerminalSessionKey::new(thread_id, terminal_id);
        self.state
            .lock()
            .ok()
            .and_then(|g| g.active.get(&key).map(|e| Arc::clone(&e.output_notify)))
    }
}

fn lock_state(
    state: &StdMutex<RuntimeState>,
) -> Result<std::sync::MutexGuard<'_, RuntimeState>, ServerRuntimeError> {
    state
        .lock()
        .map_err(|_| ServerRuntimeError::Terminal("terminal mutex poisoned".to_owned()))
}
