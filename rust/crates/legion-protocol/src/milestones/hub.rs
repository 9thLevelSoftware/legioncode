//! In-memory [`MilestoneHub`] ring buffer and cursor pager
//! (`apps/server/src/milestones/MilestoneHub.ts`).

use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use super::event_enum::{MilestoneEvent, milestone_matches};
use super::stream::{
    KeptEntry, MilestoneFollowItem, MilestoneFollowStream, MilestoneSubscription,
    PositionedMilestone,
};
use super::time::{format_base36, parse_iso_epoch_ms, system_now_ms};
use super::types::{MilestoneFilter, MilestoneId};

/// Maximum kept milestone entries retained in memory for cursor catch-up.
pub const MILESTONE_KEPT: usize = 1_000;
/// Maximum remembered milestone IDs so evicted IDs are still deduplicated.
pub const MILESTONE_IDS_KEPT: usize = 4 * MILESTONE_KEPT;
/// Live subscriber broadcast buffer capacity; slower followers see a [`MilestoneFollowItem::Gap`].
pub const MILESTONE_SUBSCRIBER_BACKLOG: usize = 256;
/// Default limit for `recent` queries when `limit` is omitted.
pub const MILESTONE_RECENT_LIMIT: usize = 200;

/// Cursor and filter parameters shared by [`MilestoneHub::read`] and [`MilestoneHub::follow`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MilestoneReadOptions {
    /// Previously returned cursor, or `None` to start from live tail.
    pub cursor: Option<String>,
    /// Skip kept milestones older than `now - max_age_ms` milliseconds.
    pub max_age_ms: Option<i64>,
    /// Optional predicate narrowing returned milestones.
    pub filter: Option<MilestoneFilter>,
}

/// Options for [`MilestoneHub::recent`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MilestoneRecentOptions {
    /// Resume strictly after this remembered milestone ID.
    pub since_id: Option<MilestoneId>,
    /// Maximum returned events (clamped to `1..=MILESTONE_KEPT`, default `200`).
    pub limit: Option<usize>,
    /// Optional predicate narrowing returned milestones.
    pub filter: Option<MilestoneFilter>,
}

/// Result returned by [`MilestoneHub::recent`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MilestoneRecentPage {
    /// Matching milestones ordered oldest to newest.
    pub events: Vec<MilestoneEvent>,
    /// `true` when `since_id` is no longer kept or `limit` truncated available events.
    pub truncated: bool,
}

/// Cursor-paginated slice returned by [`MilestoneHub::read`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MilestoneReadPage {
    /// Positioned milestones ordered oldest to newest.
    pub events: Vec<PositionedMilestone>,
    /// Next cursor to pass to subsequent reads.
    pub cursor: String,
    /// `true` when events after the supplied cursor were evicted or dropped by `max_age_ms`.
    pub truncated: bool,
    /// `true` when more matching entries remain in memory after `events`.
    pub has_more: bool,
}

#[derive(Debug)]
struct HubState {
    kept: VecDeque<KeptEntry>,
    seen: HashSet<MilestoneId>,
    seen_order: VecDeque<MilestoneId>,
    seq: u64,
}

impl HubState {
    fn new() -> Self {
        Self {
            kept: VecDeque::with_capacity(MILESTONE_KEPT),
            seen: HashSet::with_capacity(MILESTONE_IDS_KEPT),
            seen_order: VecDeque::with_capacity(MILESTONE_IDS_KEPT),
            seq: 0,
        }
    }

    fn remember(&mut self, event: MilestoneEvent) -> Option<KeptEntry> {
        if self.seen.contains(event.id()) {
            return None;
        }
        let id = event.id().clone();
        self.seen.insert(id.clone());
        self.seen_order.push_back(id);
        if self.seen_order.len() > MILESTONE_IDS_KEPT
            && let Some(evicted) = self.seen_order.pop_front()
        {
            self.seen.remove(&evicted);
        }
        self.seq = self.seq.saturating_add(1);
        let entry = KeptEntry {
            event,
            seq: self.seq,
        };
        if self.kept.len() >= MILESTONE_KEPT {
            let _ = self.kept.pop_front();
        }
        self.kept.push_back(entry.clone());
        Some(entry)
    }
}

type ClockFn = Arc<dyn Fn() -> i64 + Send + Sync>;

/// In-memory deduplicating ring buffer and live broadcast hub for milestones.
#[derive(Clone)]
pub struct MilestoneHub {
    epoch: String,
    clock: ClockFn,
    state: Arc<Mutex<HubState>>,
    tx: broadcast::Sender<KeptEntry>,
}

impl std::fmt::Debug for MilestoneHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let head = self
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .seq;
        f.debug_struct("MilestoneHub")
            .field("epoch", &self.epoch)
            .field("seq", &head)
            .finish_non_exhaustive()
    }
}

impl Default for MilestoneHub {
    fn default() -> Self {
        Self::new()
    }
}

impl MilestoneHub {
    /// Creates a new [`MilestoneHub`] using the current wall clock.
    pub fn new() -> Self {
        let now = system_now_ms();
        Self::with_clock(now, Arc::new(system_now_ms))
    }

    /// Creates a [`MilestoneHub`] with an explicit startup epoch seed and injectable clock.
    pub fn with_clock(epoch_seed_ms: i64, clock: ClockFn) -> Self {
        let seed_u64 = u64::try_from(epoch_seed_ms.max(0)).unwrap_or(0);
        let (tx, _) = broadcast::channel(MILESTONE_SUBSCRIBER_BACKLOG);
        Self {
            epoch: format_base36(seed_u64),
            clock,
            state: Arc::new(Mutex::new(HubState::new())),
            tx,
        }
    }

    /// Returns the hub's base-36 epoch prefix used in cursors.
    pub fn epoch(&self) -> &str {
        &self.epoch
    }

    fn cursor_at(&self, seq: u64) -> String {
        format!("{}.{seq}", self.epoch)
    }

    fn seq_of(&self, cursor: &str, head_seq: u64) -> Option<u64> {
        let (cursor_epoch, raw_seq) = cursor.split_once('.')?;
        if cursor_epoch != self.epoch || raw_seq.is_empty() {
            return None;
        }
        let value = raw_seq.parse::<u64>().ok()?;
        (value <= head_seq).then_some(value)
    }

    fn after(
        &self,
        state: &HubState,
        options: &MilestoneReadOptions,
        now_ms: i64,
    ) -> (Vec<KeptEntry>, bool) {
        let Some(cursor) = options.cursor.as_deref() else {
            return (Vec::new(), false);
        };
        let from = self.seq_of(cursor, state.seq);
        let oldest = state
            .kept
            .front()
            .map_or_else(|| state.seq.saturating_add(1), |entry| entry.seq);
        let mut entries: Vec<KeptEntry> = match from {
            None => state.kept.iter().cloned().collect(),
            Some(seq) => state
                .kept
                .iter()
                .filter(|entry| entry.seq > seq)
                .cloned()
                .collect(),
        };
        let mut truncated = from.is_none_or(|seq| seq.saturating_add(1) < oldest);
        if let Some(max_age_ms) = options.max_age_ms {
            let floor = now_ms.saturating_sub(max_age_ms);
            let before_len = entries.len();
            entries.retain(|entry| {
                parse_iso_epoch_ms(entry.event.occurred_at())
                    .is_some_and(|occurred| occurred >= floor)
            });
            if entries.len() < before_len {
                truncated = true;
            }
        }
        (entries, truncated)
    }

    /// Publishes `event` if its ID has not been remembered recently.
    pub fn publish(&self, event: MilestoneEvent) {
        let mut guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(entry) = guard.remember(event) else {
            return;
        };
        // Sending under lock guarantees live subscribers observe events in strict `seq` order.
        let _ = self.tx.send(entry);
    }

    /// Subscribes to newly published matching milestones from this point forward.
    pub fn subscribe(&self, filter: Option<MilestoneFilter>) -> MilestoneSubscription {
        MilestoneSubscription {
            rx: self.tx.subscribe(),
            filter,
        }
    }

    /// Reads recent kept milestones oldest-first (`since_id` or trailing newest `limit`).
    pub fn recent(&self, options: &MilestoneRecentOptions) -> MilestoneRecentPage {
        let limit = options
            .limit
            .unwrap_or(MILESTONE_RECENT_LIMIT)
            .clamp(1, MILESTONE_KEPT);
        let guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let found_idx = options.since_id.as_ref().and_then(|target| {
            guard
                .kept
                .iter()
                .position(|entry| entry.event.id() == target)
        });
        let gap = options.since_id.is_some() && found_idx.is_none();
        let start = found_idx.map_or(0, |idx| idx.saturating_add(1));
        let matching: Vec<MilestoneEvent> = guard
            .kept
            .iter()
            .skip(start)
            .map(|entry| entry.event.clone())
            .filter(|event| milestone_matches(event, options.filter.as_ref()))
            .collect();
        let events: Vec<MilestoneEvent> = if options.since_id.is_some() && !gap {
            matching.iter().take(limit).cloned().collect()
        } else {
            let skip = matching.len().saturating_sub(limit);
            matching.iter().skip(skip).cloned().collect()
        };
        let truncated = gap || events.len() < matching.len();
        MilestoneRecentPage { events, truncated }
    }

    /// Reads up to `limit` matching kept milestones after `options.cursor`.
    pub fn read(&self, options: &MilestoneReadOptions, limit: usize) -> MilestoneReadPage {
        let now_ms = (self.clock)();
        let guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        self.read_locked(&guard, options, limit, now_ms)
    }

    fn read_locked(
        &self,
        guard: &HubState,
        options: &MilestoneReadOptions,
        limit: usize,
        now_ms: i64,
    ) -> MilestoneReadPage {
        let (entries, truncated) = self.after(guard, options, now_ms);
        let cap = limit.max(1);
        let mut matching = entries
            .into_iter()
            .filter(|entry| milestone_matches(&entry.event, options.filter.as_ref()));
        let mut chosen = Vec::new();
        for entry in matching.by_ref().take(cap) {
            chosen.push(entry);
        }
        let has_more = matching.next().is_some();
        let cursor = if has_more {
            chosen.last().map_or_else(
                || self.cursor_at(guard.seq),
                |last| self.cursor_at(last.seq),
            )
        } else {
            self.cursor_at(guard.seq)
        };
        let events = chosen
            .into_iter()
            .map(|entry| PositionedMilestone {
                event: entry.event,
                cursor: self.cursor_at(entry.seq),
            })
            .collect();
        MilestoneReadPage {
            events,
            cursor,
            truncated,
            has_more,
        }
    }

    /// Replays matching kept milestones after `options.cursor`, then streams live arrivals.
    pub fn follow(&self, options: MilestoneReadOptions) -> MilestoneFollowStream {
        let now_ms = (self.clock)();
        let guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let rx = self.tx.subscribe();
        let caught = self.read_locked(&guard, &options, MILESTONE_KEPT, now_ms);
        drop(guard);

        let first_seq = caught
            .events
            .first()
            .and_then(|item| item.cursor.split_once('.'))
            .and_then(|(_, raw)| raw.parse::<u64>().ok());
        let cursor = first_seq.map_or_else(
            || caught.cursor.clone(),
            |seq| self.cursor_at(seq.saturating_sub(1)),
        );
        let last_seq = caught
            .cursor
            .split_once('.')
            .and_then(|(_, raw)| raw.parse::<u64>().ok())
            .unwrap_or(0);

        let mut replay = VecDeque::new();
        let last_replayed_cursor = caught.events.last().map(|ev| ev.cursor.clone());
        for ev in caught.events {
            replay.push_back(MilestoneFollowItem::Milestone(Box::new(ev)));
        }
        if let Some(last_cursor) = last_replayed_cursor
            && last_cursor != caught.cursor
        {
            replay.push_back(MilestoneFollowItem::Position {
                cursor: caught.cursor,
            });
        }

        MilestoneFollowStream {
            epoch: self.epoch.clone(),
            cursor,
            truncated: caught.truncated,
            filter: options.filter,
            replay,
            last_seq,
            rx,
        }
    }
}
