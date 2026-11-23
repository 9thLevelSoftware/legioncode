//! Live subscription and replay+follow stream types for [`super::hub::MilestoneHub`].

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use super::event_enum::{MilestoneEvent, milestone_matches};
use super::types::MilestoneFilter;

/// Milestone paired with the cursor that resumes immediately after it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PositionedMilestone {
    /// Emitted milestone payload.
    pub event: MilestoneEvent,
    /// Cursor pointing at this entry's sequence number (`<epoch>.<seq>`).
    pub cursor: String,
}

/// Stream element emitted by [`MilestoneFollowStream`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "_tag", rename_all = "PascalCase")]
pub enum MilestoneFollowItem {
    /// A matching milestone together with its cursor.
    Milestone(Box<PositionedMilestone>),
    /// Server sequence advanced past unmatched events; records the latest cursor.
    Position {
        /// Cursor after skipped events.
        cursor: String,
    },
    /// Slow follower fell behind subscriber backlog and skipped missed events.
    Gap {
        /// Cursor at the boundary immediately preceding the first delivered item after the gap.
        cursor: String,
    },
}

#[derive(Debug, Clone)]
pub(super) struct KeptEntry {
    pub event: MilestoneEvent,
    pub seq: u64,
}

/// Live broadcast subscription created by [`super::hub::MilestoneHub::subscribe`].
pub struct MilestoneSubscription {
    pub(super) rx: broadcast::Receiver<KeptEntry>,
    pub(super) filter: Option<MilestoneFilter>,
}

impl MilestoneSubscription {
    /// Awaits the next matching live milestone, skipping lagged items if behind buffer.
    pub async fn next(&mut self) -> Option<MilestoneEvent> {
        loop {
            match self.rx.recv().await {
                Ok(entry) => {
                    if milestone_matches(&entry.event, self.filter.as_ref()) {
                        return Some(entry.event);
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }

    /// Returns an already buffered matching milestone without waiting.
    pub fn try_next(&mut self) -> Option<MilestoneEvent> {
        loop {
            match self.rx.try_recv() {
                Ok(entry) => {
                    if milestone_matches(&entry.event, self.filter.as_ref()) {
                        return Some(entry.event);
                    }
                }
                Err(broadcast::error::TryRecvError::Lagged(_)) => {}
                Err(
                    broadcast::error::TryRecvError::Empty | broadcast::error::TryRecvError::Closed,
                ) => return None,
            }
        }
    }
}

/// Replay + live follower returned by [`super::hub::MilestoneHub::follow`].
pub struct MilestoneFollowStream {
    pub(super) epoch: String,
    /// Starting cursor immediately preceding the first replayed item (or head cursor).
    pub cursor: String,
    /// `true` when historical milestones before replay were evicted or aged out.
    pub truncated: bool,
    pub(super) filter: Option<MilestoneFilter>,
    pub(super) replay: VecDeque<MilestoneFollowItem>,
    pub(super) last_seq: u64,
    pub(super) rx: broadcast::Receiver<KeptEntry>,
}

impl MilestoneFollowStream {
    /// Awaits the next replay or live [`MilestoneFollowItem`].
    pub async fn next(&mut self) -> Option<MilestoneFollowItem> {
        if let Some(item) = self.replay.pop_front() {
            return Some(item);
        }
        loop {
            match self.rx.recv().await {
                Ok(entry) => {
                    if let Some(item) = self.push_live_entry(entry) {
                        return Some(item);
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }

    /// Drains one already buffered replay or live [`MilestoneFollowItem`] without waiting.
    pub fn try_next(&mut self) -> Option<MilestoneFollowItem> {
        if let Some(item) = self.replay.pop_front() {
            return Some(item);
        }
        loop {
            match self.rx.try_recv() {
                Ok(entry) => {
                    if let Some(item) = self.push_live_entry(entry) {
                        return Some(item);
                    }
                }
                Err(broadcast::error::TryRecvError::Lagged(_)) => {}
                Err(
                    broadcast::error::TryRecvError::Empty | broadcast::error::TryRecvError::Closed,
                ) => return None,
            }
        }
    }

    fn push_live_entry(&mut self, entry: KeptEntry) -> Option<MilestoneFollowItem> {
        if entry.seq <= self.last_seq {
            return None;
        }
        let gap = entry.seq > self.last_seq.saturating_add(1);
        self.last_seq = entry.seq;
        let cursor = format!("{}.{}", self.epoch, entry.seq);
        let item = if milestone_matches(&entry.event, self.filter.as_ref()) {
            MilestoneFollowItem::Milestone(Box::new(PositionedMilestone {
                event: entry.event,
                cursor,
            }))
        } else {
            MilestoneFollowItem::Position { cursor }
        };
        if gap {
            self.replay.push_back(item);
            return Some(MilestoneFollowItem::Gap {
                cursor: format!("{}.{}", self.epoch, entry.seq.saturating_sub(1)),
            });
        }
        Some(item)
    }
}
