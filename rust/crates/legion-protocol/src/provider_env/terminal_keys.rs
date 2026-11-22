//! Terminal resumption key tracking distinguishing LRU eviction from explicit close.
//!
//! Fixes BUG-08 in `apps/server/src/terminal/Manager.ts`: inactive terminals evicted
//! under the LRU retention ceiling enter `evicted` rather than `closed`, keeping
//! them resumable until explicitly closed.

use std::collections::{HashSet, VecDeque};

/// Composite `(thread_id, terminal_id)` key identifying a persisted PTY session.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TerminalSessionKey {
    /// Owning thread identifier.
    pub thread_id: String,
    /// Thread-local terminal tab identifier.
    pub terminal_id: String,
}

impl TerminalSessionKey {
    /// Builds a [`TerminalSessionKey`] from `thread_id` and `terminal_id`.
    pub fn new(thread_id: impl Into<String>, terminal_id: impl Into<String>) -> Self {
        Self {
            thread_id: thread_id.into(),
            terminal_id: terminal_id.into(),
        }
    }

    /// Encodes the NUL-delimited internal map key (`thread_id\0terminal_id`).
    pub fn to_wire_key(&self) -> String {
        format!("{}\0{}", self.thread_id, self.terminal_id)
    }

    /// Parses a NUL-delimited `thread_id\0terminal_id` wire key if well-formed.
    pub fn from_wire_key(wire: &str) -> Option<Self> {
        let (thread_id, terminal_id) = wire.split_once('\0')?;
        if thread_id.is_empty() || terminal_id.is_empty() {
            return None;
        }
        Some(Self {
            thread_id: thread_id.to_owned(),
            terminal_id: terminal_id.to_owned(),
        })
    }
}

/// Resumption eligibility state reported by [`TerminalSessionKeys::status`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalResumeStatus {
    /// Session is actively attached or hot in inactive LRU retention.
    ActiveOrRetained,
    /// Session overflowed the inactive LRU ceiling and was evicted, but remains resumable.
    EvictedResumable,
    /// Session was explicitly closed by user or lifecycle command and must not auto-resume.
    Closed,
    /// Session key has not been recorded; clean open is permitted.
    Unknown,
}

/// Tracks active/inactive retention separately from `evicted` and `closed` terminal sets.
#[derive(Debug, Clone)]
pub struct TerminalSessionKeys {
    max_inactive_retained: usize,
    active: HashSet<TerminalSessionKey>,
    inactive_lru: VecDeque<TerminalSessionKey>,
    evicted: HashSet<TerminalSessionKey>,
    closed: HashSet<TerminalSessionKey>,
}

impl Default for TerminalSessionKeys {
    fn default() -> Self {
        Self::with_capacity(64)
    }
}

impl TerminalSessionKeys {
    /// Creates a [`TerminalSessionKeys`] tracker retaining up to `max_inactive_retained` hot shells.
    pub fn with_capacity(max_inactive_retained: usize) -> Self {
        Self {
            max_inactive_retained,
            active: HashSet::new(),
            inactive_lru: VecDeque::new(),
            evicted: HashSet::new(),
            closed: HashSet::new(),
        }
    }

    /// Marks `key` active and clears any prior `evicted` or `closed` state.
    pub fn open(&mut self, key: TerminalSessionKey) {
        self.closed.remove(&key);
        self.evicted.remove(&key);
        self.inactive_lru.retain(|item| item != &key);
        self.active.insert(key);
    }

    /// Moves `key` into the inactive LRU queue, spilling oldest entries into `evicted` (not `closed`).
    pub fn mark_inactive(&mut self, key: TerminalSessionKey) -> Vec<TerminalSessionKey> {
        if self.closed.contains(&key) {
            return Vec::new();
        }
        self.active.remove(&key);
        self.evicted.remove(&key);
        self.inactive_lru.retain(|item| item != &key);
        self.inactive_lru.push_back(key);

        let mut evicted_now = Vec::new();
        while self.inactive_lru.len() > self.max_inactive_retained {
            if let Some(oldest) = self.inactive_lru.pop_front() {
                self.evicted.insert(oldest.clone());
                evicted_now.push(oldest);
            }
        }
        evicted_now
    }

    /// Explicitly marks `key` as LRU-evicted without marking it `closed` (BUG-08).
    pub fn evict_inactive(&mut self, key: &TerminalSessionKey) -> bool {
        if self.closed.contains(key) {
            return false;
        }
        self.active.remove(key);
        self.inactive_lru.retain(|item| item != key);
        self.evicted.insert(key.clone())
    }

    /// Permanently closes `key`, preventing subsequent implicit resumption.
    pub fn close(&mut self, key: TerminalSessionKey) {
        self.active.remove(&key);
        self.inactive_lru.retain(|item| item != &key);
        self.evicted.remove(&key);
        self.closed.insert(key);
    }

    /// Returns `true` when `key` has not been explicitly closed (evicted sessions stay resumable).
    pub fn can_resume(&self, key: &TerminalSessionKey) -> bool {
        !self.closed.contains(key)
    }

    /// Returns `true` when `key` was explicitly closed.
    pub fn is_closed(&self, key: &TerminalSessionKey) -> bool {
        self.closed.contains(key)
    }

    /// Returns `true` when `key` was LRU-evicted and remains resumable.
    pub fn is_evicted(&self, key: &TerminalSessionKey) -> bool {
        self.evicted.contains(key)
    }

    /// Returns the detailed [`TerminalResumeStatus`] of `key`.
    pub fn status(&self, key: &TerminalSessionKey) -> TerminalResumeStatus {
        if self.closed.contains(key) {
            TerminalResumeStatus::Closed
        } else if self.evicted.contains(key) {
            TerminalResumeStatus::EvictedResumable
        } else if self.active.contains(key) || self.inactive_lru.contains(key) {
            TerminalResumeStatus::ActiveOrRetained
        } else {
            TerminalResumeStatus::Unknown
        }
    }

    /// Returns counts `(active, inactive_retained, evicted, closed)`.
    pub fn counts(&self) -> (usize, usize, usize, usize) {
        (
            self.active.len(),
            self.inactive_lru.len(),
            self.evicted.len(),
            self.closed.len(),
        )
    }
}
