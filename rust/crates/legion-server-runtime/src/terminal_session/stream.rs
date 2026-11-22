//! Per-session terminal stream state managing sanitized scrollback history and broadcast delivery.

use legion_terminal_vt::TerminalHistorySanitizer;
use tokio::sync::broadcast;

use crate::outbox_state::utc_now_iso;
use crate::terminal_session::stream_types::{
    DEFAULT_STREAM_CHANNEL_CAPACITY, TerminalMetadataStreamEvent, TerminalOutputSubscription,
    TerminalSessionLifecycleStatus, TerminalSessionStateSnapshot, TerminalSessionSummary,
    TerminalStreamChunk, retain_history_tail,
};

pub(crate) struct TerminalStreamState {
    thread_id: String,
    terminal_id: String,
    cwd: String,
    worktree_path: Option<String>,
    status: TerminalSessionLifecycleStatus,
    pid: Option<u32>,
    exit_code: Option<i32>,
    exit_signal: Option<i32>,
    label: String,
    updated_at: String,
    sequence: u64,
    cols: u16,
    rows: u16,
    error: Option<String>,
    max_history_bytes: usize,
    history: String,
    history_sanitizer: TerminalHistorySanitizer,
    pending_utf8: Vec<u8>,
    tx: broadcast::Sender<TerminalStreamChunk>,
    metadata_tx: broadcast::Sender<TerminalMetadataStreamEvent>,
}

impl TerminalStreamState {
    #[expect(
        clippy::too_many_arguments,
        reason = "initializes all session stream fields atomically once at open"
    )]
    pub(crate) fn new(
        thread_id: String,
        terminal_id: String,
        cwd: String,
        worktree_path: Option<String>,
        label: String,
        cols: u16,
        rows: u16,
        pid: Option<u32>,
        max_history_bytes: usize,
        metadata_tx: broadcast::Sender<TerminalMetadataStreamEvent>,
    ) -> Self {
        let (tx, _) = broadcast::channel(DEFAULT_STREAM_CHANNEL_CAPACITY);
        Self {
            thread_id,
            terminal_id,
            cwd,
            worktree_path,
            status: TerminalSessionLifecycleStatus::Running,
            pid,
            exit_code: None,
            exit_signal: None,
            label,
            updated_at: utc_now_iso(),
            sequence: 0,
            cols,
            rows,
            error: None,
            max_history_bytes: max_history_bytes.max(1),
            history: String::new(),
            history_sanitizer: TerminalHistorySanitizer::new(),
            pending_utf8: Vec::new(),
            tx,
            metadata_tx,
        }
    }

    pub(crate) fn snapshot(
        &self,
        max_history_override: Option<usize>,
    ) -> TerminalSessionStateSnapshot {
        let history = match max_history_override {
            Some(limit) => retain_history_tail(&self.history, limit),
            None => self.history.clone(),
        };
        TerminalSessionStateSnapshot {
            thread_id: self.thread_id.clone(),
            terminal_id: self.terminal_id.clone(),
            cwd: self.cwd.clone(),
            worktree_path: self.worktree_path.clone(),
            status: self.status,
            pid: self.pid,
            history,
            exit_code: self.exit_code,
            exit_signal: self.exit_signal,
            label: self.label.clone(),
            updated_at: self.updated_at.clone(),
            sequence: self.sequence,
            cols: self.cols,
            rows: self.rows,
            error: self.error.clone(),
        }
    }

    pub(crate) fn summary(&self) -> TerminalSessionSummary {
        TerminalSessionSummary {
            thread_id: self.thread_id.clone(),
            terminal_id: self.terminal_id.clone(),
            cwd: self.cwd.clone(),
            worktree_path: self.worktree_path.clone(),
            status: self.status,
            pid: self.pid,
            exit_code: self.exit_code,
            exit_signal: self.exit_signal,
            has_running_subprocess: self.status == TerminalSessionLifecycleStatus::Running
                && self.pid.is_some(),
            label: self.label.clone(),
            updated_at: self.updated_at.clone(),
        }
    }

    pub(crate) fn subscribe(
        &self,
        max_history_override: Option<usize>,
    ) -> TerminalOutputSubscription {
        let receiver = self.tx.subscribe();
        let snapshot = self.snapshot(max_history_override);
        TerminalOutputSubscription { snapshot, receiver }
    }

    pub(crate) const fn set_pid(&mut self, pid: Option<u32>) {
        self.pid = pid;
    }

    fn next_seq(&mut self) -> u64 {
        self.sequence = self.sequence.saturating_add(1);
        self.updated_at = utc_now_iso();
        self.sequence
    }

    pub(crate) fn emit_started(&mut self, is_restart: bool) {
        let sequence = self.next_seq();
        let snapshot = self.snapshot(None);
        let chunk = if is_restart {
            TerminalStreamChunk::Restarted {
                thread_id: self.thread_id.clone(),
                terminal_id: self.terminal_id.clone(),
                sequence,
                snapshot,
            }
        } else {
            TerminalStreamChunk::Started {
                thread_id: self.thread_id.clone(),
                terminal_id: self.terminal_id.clone(),
                sequence,
                snapshot,
            }
        };
        let _ = self.tx.send(chunk);
        let _ = self.metadata_tx.send(TerminalMetadataStreamEvent::Upsert {
            terminal: self.summary(),
        });
    }

    pub(crate) fn push_raw_output(&mut self, bytes: &[u8], title_from_grid: Option<&str>) {
        if bytes.is_empty() && title_from_grid.is_none() {
            return;
        }
        let decoded = decode_streaming_utf8(&mut self.pending_utf8, bytes);
        if !decoded.is_empty() || !bytes.is_empty() {
            let sanitized = self.history_sanitizer.push_chunk(&decoded);
            if !sanitized.visible_text.is_empty() {
                self.history.push_str(&sanitized.visible_text);
                if self.history.len() > self.max_history_bytes {
                    self.history = retain_history_tail(&self.history, self.max_history_bytes);
                }
            }
            let sequence = self.next_seq();
            let _ = self.tx.send(TerminalStreamChunk::Output {
                thread_id: self.thread_id.clone(),
                terminal_id: self.terminal_id.clone(),
                sequence,
                data: decoded,
                bytes: bytes.to_vec(),
            });
        }
        if let Some(title) = title_from_grid
            && !title.is_empty()
            && title != self.label
        {
            self.label = truncate_label(title);
            let sequence = self.next_seq();
            let _ = self.tx.send(TerminalStreamChunk::Activity {
                thread_id: self.thread_id.clone(),
                terminal_id: self.terminal_id.clone(),
                sequence,
                has_running_subprocess: self.status == TerminalSessionLifecycleStatus::Running
                    && self.pid.is_some(),
                label: self.label.clone(),
                cols: None,
                rows: None,
            });
            let _ = self.metadata_tx.send(TerminalMetadataStreamEvent::Upsert {
                terminal: self.summary(),
            });
        }
    }

    pub(crate) fn flush_eof_utf8(&mut self) {
        if !self.pending_utf8.is_empty() {
            let stray = std::mem::take(&mut self.pending_utf8);
            let tail = String::from_utf8_lossy(&stray).into_owned();
            if !tail.is_empty() {
                let sanitized = self.history_sanitizer.push_chunk(&tail);
                self.history.push_str(&sanitized.visible_text);
                let sequence = self.next_seq();
                let _ = self.tx.send(TerminalStreamChunk::Output {
                    thread_id: self.thread_id.clone(),
                    terminal_id: self.terminal_id.clone(),
                    sequence,
                    data: tail,
                    bytes: stray,
                });
            }
        }
        let leftover = self.history_sanitizer.finish();
        if !leftover.is_empty() {
            self.history.push_str(&leftover);
        }
        if self.history.len() > self.max_history_bytes {
            self.history = retain_history_tail(&self.history, self.max_history_bytes);
        }
    }

    pub(crate) fn mark_resize(&mut self, cols: u16, rows: u16) {
        self.cols = cols;
        self.rows = rows;
        let sequence = self.next_seq();
        let _ = self.tx.send(TerminalStreamChunk::Activity {
            thread_id: self.thread_id.clone(),
            terminal_id: self.terminal_id.clone(),
            sequence,
            has_running_subprocess: self.status == TerminalSessionLifecycleStatus::Running
                && self.pid.is_some(),
            label: self.label.clone(),
            cols: Some(cols),
            rows: Some(rows),
        });
    }

    pub(crate) fn mark_cleared(&mut self) {
        self.history.clear();
        self.pending_utf8.clear();
        self.history_sanitizer = TerminalHistorySanitizer::new();
        let sequence = self.next_seq();
        let _ = self.tx.send(TerminalStreamChunk::Cleared {
            thread_id: self.thread_id.clone(),
            terminal_id: self.terminal_id.clone(),
            sequence,
        });
    }

    pub(crate) fn mark_exited(&mut self, exit_code: Option<i32>, exit_signal: Option<i32>) {
        if self.status == TerminalSessionLifecycleStatus::Exited {
            return;
        }
        if self.status != TerminalSessionLifecycleStatus::Error {
            self.status = TerminalSessionLifecycleStatus::Exited;
        }
        self.pid = None;
        self.exit_code = exit_code;
        self.exit_signal = exit_signal;
        let sequence = self.next_seq();
        let _ = self.tx.send(TerminalStreamChunk::Exited {
            thread_id: self.thread_id.clone(),
            terminal_id: self.terminal_id.clone(),
            sequence,
            exit_code,
            exit_signal,
        });
        let _ = self.metadata_tx.send(TerminalMetadataStreamEvent::Upsert {
            terminal: self.summary(),
        });
    }

    pub(crate) fn mark_closed(&mut self) {
        let sequence = self.next_seq();
        self.pid = None;
        let _ = self.tx.send(TerminalStreamChunk::Closed {
            thread_id: self.thread_id.clone(),
            terminal_id: self.terminal_id.clone(),
            sequence,
        });
        let _ = self.metadata_tx.send(TerminalMetadataStreamEvent::Remove {
            thread_id: self.thread_id.clone(),
            terminal_id: self.terminal_id.clone(),
        });
    }
}

fn truncate_label(raw: &str) -> String {
    let mut out = String::new();
    for ch in raw.chars() {
        if out.len() + ch.len_utf8() > 128 {
            break;
        }
        out.push(ch);
    }
    out
}

fn decode_streaming_utf8(pending: &mut Vec<u8>, bytes: &[u8]) -> String {
    pending.extend_from_slice(bytes);
    let mut out = String::new();
    loop {
        match std::str::from_utf8(pending) {
            Ok(valid) => {
                out.push_str(valid);
                pending.clear();
                break;
            }
            Err(err) => {
                let valid_up_to = err.valid_up_to();
                if valid_up_to > 0
                    && let Ok(valid) = std::str::from_utf8(&pending[..valid_up_to])
                {
                    out.push_str(valid);
                }
                if let Some(error_len) = err.error_len() {
                    out.push('\u{FFFD}');
                    pending.drain(..valid_up_to + error_len);
                } else {
                    pending.drain(..valid_up_to);
                    break;
                }
            }
        }
    }
    out
}
