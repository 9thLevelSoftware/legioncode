//! Read-only linked-thread history pagination & reconnect retention state (`#40545`,
//! `packages/client-runtime/src/linkedThread.ts`).

use serde::{Deserialize, Serialize};

/// Minimal identity tuple of an older loaded turn item preserved across snapshot reconnects.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryAnchor {
    /// Thread that owns `source_item_id`.
    pub source_thread_id: String,
    /// Projected item ID used to detect when backfill has recovered the prior visible prefix.
    pub source_item_id: String,
}

/// Snapshot sequence paired with its `history_cursor` (`LinkedHistoryWindow`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkedHistoryWindow {
    /// Snapshot sequence number at which `cursor` is valid.
    pub sequence: u64,
    /// History pagination cursor returned alongside the snapshot.
    pub cursor: String,
}

/// Persisted linked history pagination & recovery checkpoint (`LinkedThreadView.loadedHistory`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadedLinkedHistory {
    /// Snapshot window on which pages were loaded.
    pub window: LinkedHistoryWindow,
    /// Next page cursor remaining after loaded pages, or `None` once exhausted.
    pub next_cursor: Option<String>,
    /// Oldest item anchor still needing recovery if a snapshot jumps ahead before backfill finishes.
    pub restore_through: Option<HistoryAnchor>,
}

/// Snapshot summary fed into [`evaluate_linked_snapshot_retention`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedSnapshotInput {
    /// Snapshot sequence number.
    pub sequence: u64,
    /// Snapshot initial `historyCursor`.
    pub history_cursor: Option<String>,
    /// Visible items carried inside this snapshot (oldest first).
    pub visible_items: Vec<HistoryAnchor>,
}

/// History page summary fed into [`evaluate_linked_history_page`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedHistoryPageInput {
    /// Sequence number at which the peer generated this history page.
    pub snapshot_sequence: u64,
    /// Whether the peer reports older pages before this one.
    pub has_more_history: bool,
    /// Cursor for the next older page, if any.
    pub next_cursor: Option<String>,
    /// Combined visible items (oldest first) after applying this page.
    pub combined_visible_items: Vec<HistoryAnchor>,
}

/// Action returned by [`evaluate_linked_snapshot_retention`] and [`evaluate_linked_history_page`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkedHistoryDecision {
    /// Reuse held older rows from the previous view immediately (snapshot kept window compatible).
    PrependHeldRows {
        /// Restored `nextCursor` from `loadedHistory`.
        next_cursor: Option<String>,
        /// Updated [`LoadedLinkedHistory`] to store.
        loaded_history: LoadedLinkedHistory,
    },
    /// Schedule background `historyPage(cursor)` backfill to reach `restore_through`.
    FetchPage {
        /// Target snapshot window.
        window: LinkedHistoryWindow,
        /// Page cursor to fetch next.
        cursor: String,
        /// Updated `loadedHistory` with `restoreThrough` anchor.
        loaded_history: LoadedLinkedHistory,
    },
    /// Pagination finished cleanly or was reset without needing automatic backfill.
    Settled {
        /// Updated `loadedHistory`, if any.
        loaded_history: Option<LoadedLinkedHistory>,
    },
    /// Ignore stale/invalid page result and mark `history.failed = true`.
    MarkPageFailed,
    /// Ignore stale page response from a superseded window or cursor.
    StaleIgnored,
}

/// Returns `true` when `rows` contains `anchor`.
#[must_use]
pub fn includes_history_anchor(rows: &[HistoryAnchor], anchor: &HistoryAnchor) -> bool {
    rows.iter().any(|row| row == anchor)
}

/// Retains `kept`'s projection (clearing stale `history_cursor`) and `loaded_history` when `next`
/// reconnects without a projection yet (`keepLinkedThreadView` in `#40545`).
#[must_use]
pub fn keep_loaded_history_on_reconnect(
    next_has_projection: bool,
    kept_has_projection: bool,
    kept_loaded_history: Option<LoadedLinkedHistory>,
) -> Option<LoadedLinkedHistory> {
    if !next_has_projection && kept_has_projection {
        kept_loaded_history
    } else {
        None
    }
}

/// Evaluates how `on_snapshot` should retain or restore previously loaded older pages (`#40545`).
#[must_use]
pub fn evaluate_linked_snapshot_retention(
    incoming: &LinkedSnapshotInput,
    kept_sequence: Option<u64>,
    kept_oldest_visible: Option<&HistoryAnchor>,
    kept_loaded: Option<&LoadedLinkedHistory>,
) -> LinkedHistoryDecision {
    let Some(cursor) = incoming.history_cursor.clone() else {
        return LinkedHistoryDecision::Settled {
            loaded_history: None,
        };
    };
    let window = LinkedHistoryWindow {
        sequence: incoming.sequence,
        cursor,
    };
    let (Some(kept_seq), Some(loaded)) = (kept_sequence, kept_loaded) else {
        return LinkedHistoryDecision::Settled {
            loaded_history: None,
        };
    };

    // Live updates at the same revision or matching cursor keep held rows without re-fetching.
    if kept_seq == window.sequence
        && (window.sequence > loaded.window.sequence || window.cursor == loaded.window.cursor)
    {
        return LinkedHistoryDecision::PrependHeldRows {
            next_cursor: loaded.next_cursor.clone(),
            loaded_history: LoadedLinkedHistory {
                window,
                next_cursor: loaded.next_cursor.clone(),
                restore_through: loaded.restore_through.clone(),
            },
        };
    }

    if window.sequence > kept_seq {
        let oldest = loaded.restore_through.as_ref().or(kept_oldest_visible);
        if let Some(anchor) = oldest
            && !includes_history_anchor(&incoming.visible_items, anchor)
        {
            let next_cursor = window.cursor.clone();
            let loaded_history = LoadedLinkedHistory {
                window: window.clone(),
                next_cursor: Some(next_cursor.clone()),
                restore_through: Some(anchor.clone()),
            };
            return LinkedHistoryDecision::FetchPage {
                window,
                cursor: next_cursor,
                loaded_history,
            };
        }
    }

    LinkedHistoryDecision::Settled {
        loaded_history: None,
    }
}

/// Evaluates an incoming `historyPage` result against the active window & `restore_through` anchor (`#40545`).
#[must_use]
pub fn evaluate_linked_history_page(
    active_window: &LinkedHistoryWindow,
    requested_window: &LinkedHistoryWindow,
    requested_cursor: &str,
    current_history_cursor: Option<&str>,
    current_restore_through: Option<&HistoryAnchor>,
    page: Option<&LinkedHistoryPageInput>,
) -> LinkedHistoryDecision {
    if active_window != requested_window || current_history_cursor != Some(requested_cursor) {
        return LinkedHistoryDecision::StaleIgnored;
    }
    let Some(page) = page else {
        return LinkedHistoryDecision::MarkPageFailed;
    };
    if page.snapshot_sequence < requested_window.sequence
        || (page.has_more_history && page.next_cursor.as_deref() == Some(requested_cursor))
    {
        return LinkedHistoryDecision::MarkPageFailed;
    }

    let next_cursor = if page.has_more_history {
        page.next_cursor.clone()
    } else {
        None
    };
    let mut restore_through = current_restore_through.cloned();
    if let Some(anchor) = &restore_through
        && (next_cursor.is_none() || includes_history_anchor(&page.combined_visible_items, anchor))
    {
        restore_through = None;
    }

    let loaded_history = LoadedLinkedHistory {
        window: requested_window.clone(),
        next_cursor: next_cursor.clone(),
        restore_through: restore_through.clone(),
    };
    if let (Some(_anchor), Some(next)) = (&restore_through, &next_cursor) {
        return LinkedHistoryDecision::FetchPage {
            window: requested_window.clone(),
            cursor: next.clone(),
            loaded_history,
        };
    }

    LinkedHistoryDecision::Settled {
        loaded_history: Some(loaded_history),
    }
}
