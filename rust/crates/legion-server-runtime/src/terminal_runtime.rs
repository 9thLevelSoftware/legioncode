//! Live terminal session manager wiring `TerminalSessionKeys` (`BUG-08`), `VtParser`, `AnsiPalette256` (`BUG-25`), and `"LCVT"` snapshots.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};

use legion_protocol::provider_env::{
    TerminalResumeStatus, TerminalSessionKey, TerminalSessionKeys,
};
use legion_terminal_vt::{Rgba, TerminalGrid, VtParser, encode_lcvt};
use tokio::sync::Notify;

use crate::error::ServerRuntimeError;
use crate::terminal_session::{
    AttachedProcess, EvictedSessionMeta, TerminalOpenRequest, TerminalOpenResult,
    TerminalSessionEntry, TerminalSnapshotResult, prepare_write_bytes,
};

struct RuntimeState {
    keys: TerminalSessionKeys,
    active: HashMap<TerminalSessionKey, TerminalSessionEntry>,
    evicted_meta: HashMap<TerminalSessionKey, EvictedSessionMeta>,
}

/// Thread-safe live terminal runtime managing VT grids, palettes, snapshots, and child processes.
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
        Self {
            state: Arc::new(StdMutex::new(RuntimeState {
                keys: TerminalSessionKeys::with_capacity(max_inactive_retained),
                active: HashMap::new(),
                evicted_meta: HashMap::new(),
            })),
        }
    }

    /// Open a new terminal session or resume a retained/evicted session (`BUG-08` forbids resuming explicitly closed sessions).
    pub fn terminal_open(
        &self,
        req: &TerminalOpenRequest,
    ) -> Result<TerminalOpenResult, ServerRuntimeError> {
        let key = TerminalSessionKey::new(&req.thread_id, &req.terminal_id);
        let mut guard = self
            .state
            .lock()
            .map_err(|_| ServerRuntimeError::Terminal("terminal mutex poisoned".to_owned()))?;

        if req.resume {
            if guard.keys.is_closed(&key) {
                return Err(ServerRuntimeError::Conflict(format!(
                    "terminal `{}` was explicitly closed and cannot be resumed",
                    key.to_wire_key()
                )));
            }
            if !guard.keys.can_resume(&key) {
                return Err(ServerRuntimeError::NotFound(format!(
                    "terminal `{}` has no resumable session state",
                    key.to_wire_key()
                )));
            }
        }

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
        let cols = meta.as_ref().map_or(req.cols, |m| m.cols);
        let rows = meta.as_ref().map_or(req.rows, |m| m.rows);
        let cwd = req
            .cwd
            .clone()
            .or_else(|| meta.as_ref().and_then(|m| m.cwd.clone()));
        let palette_overrides = meta.map_or_else(Vec::new, |m| m.palette_overrides);

        let mut grid_val = TerminalGrid::try_new(cols, rows)?;
        if !palette_overrides.is_empty() {
            grid_val
                .palette_mut()
                .apply_theme_overrides(&palette_overrides);
        }
        let grid = Arc::new(StdMutex::new(grid_val));
        let parser = Arc::new(StdMutex::new(VtParser::new()));
        let output_notify = Arc::new(Notify::new());

        let process = match &req.command {
            Some(cmd) => Some(AttachedProcess::spawn(
                cmd,
                &req.args,
                cwd.as_ref(),
                &req.env_overrides,
                &grid,
                &parser,
                &output_notify,
            )?),
            None => None,
        };

        if let Some(mut old) = guard.active.remove(&key)
            && let Some(mut proc_handle) = old.process.take()
        {
            proc_handle.terminate();
        }
        guard.active.insert(
            key.clone(),
            TerminalSessionEntry {
                grid,
                parser,
                cwd,
                palette_overrides,
                process,
                output_notify,
            },
        );
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
        let mut guard = self
            .state
            .lock()
            .map_err(|_| ServerRuntimeError::Terminal("terminal mutex poisoned".to_owned()))?;
        let evicted = guard.keys.mark_inactive(key);
        for ev_key in &evicted {
            if let Some(mut entry) = guard.active.remove(ev_key) {
                if let Some(mut proc_handle) = entry.process.take() {
                    proc_handle.terminate();
                }
                let (cols, rows) = entry.grid.lock().map_or((80, 24), |g| (g.cols(), g.rows()));
                guard.evicted_meta.insert(
                    ev_key.clone(),
                    EvictedSessionMeta {
                        cols,
                        rows,
                        cwd: entry.cwd,
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
        let mut guard = self
            .state
            .lock()
            .map_err(|_| ServerRuntimeError::Terminal("terminal mutex poisoned".to_owned()))?;
        let entry = guard.active.get_mut(&key).ok_or_else(|| {
            ServerRuntimeError::NotFound(format!("terminal `{}` is not active", key.to_wire_key()))
        })?;
        let bracketed = entry.grid.lock().is_ok_and(|g| g.bracketed_paste_enabled());
        let bytes = prepare_write_bytes(data, as_paste, bracketed);
        if let Some(proc_handle) = entry.process.as_mut() {
            proc_handle.write_bytes(&bytes)?;
        } else if let (Ok(mut g), Ok(mut p)) = (entry.grid.lock(), entry.parser.lock()) {
            p.feed(&bytes, &mut g);
            entry.output_notify.notify_waiters();
        }
        Ok(())
    }

    /// Resize the session's `TerminalGrid`.
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
        let guard = self
            .state
            .lock()
            .map_err(|_| ServerRuntimeError::Terminal("terminal mutex poisoned".to_owned()))?;
        let entry = guard.active.get(&key).ok_or_else(|| {
            ServerRuntimeError::NotFound(format!("terminal `{}` is not active", key.to_wire_key()))
        })?;
        let mut g = entry
            .grid
            .lock()
            .map_err(|_| ServerRuntimeError::Terminal("grid mutex poisoned".to_owned()))?;
        g.resize(cols, rows);
        Ok(())
    }

    /// Apply 256-color palette overrides (`BUG-25`) to an active terminal's `AnsiPalette256`.
    pub fn apply_palette_overrides(
        &self,
        thread_id: &str,
        terminal_id: &str,
        overrides: &[(u8, Rgba)],
    ) -> Result<(), ServerRuntimeError> {
        let key = TerminalSessionKey::new(thread_id, terminal_id);
        let mut guard = self
            .state
            .lock()
            .map_err(|_| ServerRuntimeError::Terminal("terminal mutex poisoned".to_owned()))?;
        let entry = guard.active.get_mut(&key).ok_or_else(|| {
            ServerRuntimeError::NotFound(format!("terminal `{}` is not active", key.to_wire_key()))
        })?;
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
        let guard = self
            .state
            .lock()
            .map_err(|_| ServerRuntimeError::Terminal("terminal mutex poisoned".to_owned()))?;
        let entry = guard.active.get(&key).ok_or_else(|| {
            ServerRuntimeError::NotFound(format!("terminal `{}` is not active", key.to_wire_key()))
        })?;
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
        let mut guard = self
            .state
            .lock()
            .map_err(|_| ServerRuntimeError::Terminal("terminal mutex poisoned".to_owned()))?;
        if let Some(mut entry) = guard.active.remove(&key)
            && let Some(mut proc_handle) = entry.process.take()
        {
            proc_handle.terminate();
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
