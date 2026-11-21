//! Context handoff token/byte budgeting and head-tail UTF-16 truncation.
//!
//! Mirrors `packages/provider-core/src/server/handoffBudget.ts` and exposes the
//! attachment formatting helpers from [`super::attachments`].

use serde::{Deserialize, Serialize};
use t3_shared::utf16::{utf8_len_to_utf16, utf16_slice_safe};

pub use super::attachments::{
    AttachmentPrompt, FormattedAttachmentPrompt, PROVIDER_SEND_TURN_MAX_INPUT_CHARS,
    PROVIDER_SEND_TURN_SUPPORTED_IMAGE_MIME_TYPES, PromptAttachment, compact_accessibility_tree,
};

/// Default handoff context token budget (`16,000` estimated tokens).
pub const DEFAULT_HANDOFF_TOKEN_CAP: usize = 16_000;
/// Lower clamp bound for caller-configured handoff token budgets.
pub const MIN_HANDOFF_TOKEN_CAP: usize = 1_024;
/// Upper clamp bound for caller-configured handoff token budgets.
pub const MAX_HANDOFF_TOKEN_CAP: usize = 64_000;
/// Default byte ceiling on serialized handoff payloads (`64 KiB`).
pub const HANDOFF_BYTE_CAP: usize = 64_000;
/// Token allowance deducted for each attached image during handoff sizing.
pub const IMAGE_ATTACHMENT_TOKEN_ALLOWANCE: usize = 8_192;
/// Token allowance deducted for each non-image file attachment during handoff sizing.
pub const FILE_ATTACHMENT_TOKEN_ALLOWANCE: usize = 4_096;
/// Maximum number of omitted message IDs listed in a handoff omission summary.
pub const MAX_OMITTED_IDS_LISTED: usize = 20;

/// Default marker inserted between retained head and tail segments.
pub const DEFAULT_TRUNCATION_MARKER: &str = "\n[Content truncated]\n";

/// Clamps a requested handoff token budget into `[MIN_HANDOFF_TOKEN_CAP, MAX_HANDOFF_TOKEN_CAP]`.
#[must_use]
pub const fn clamp_handoff_token_cap(requested: usize) -> usize {
    if requested < MIN_HANDOFF_TOKEN_CAP {
        MIN_HANDOFF_TOKEN_CAP
    } else if requested > MAX_HANDOFF_TOKEN_CAP {
        MAX_HANDOFF_TOKEN_CAP
    } else {
        requested
    }
}

/// Estimates provider token usage from UTF-8 byte length (`ceil(bytes / 4)`).
#[must_use]
pub const fn estimate_text_tokens(text: &str) -> usize {
    if text.is_empty() {
        0
    } else {
        text.len().div_ceil(4)
    }
}

/// Truncates `text` to at most `max_chars` UTF-16 code units while preserving
/// both the beginning and end around `marker` without splitting surrogate pairs.
///
/// # Examples
///
/// ```
/// use t3_providers::core::{DEFAULT_TRUNCATION_MARKER, truncate_head_tail_utf16};
/// use t3_shared::utf16::utf8_len_to_utf16;
///
/// let input = "HEAD:".to_owned() + &"a".repeat(200) + ":TAIL";
/// let truncated = truncate_head_tail_utf16(&input, 60, DEFAULT_TRUNCATION_MARKER);
/// assert!(utf8_len_to_utf16(&truncated) <= 60);
/// assert!(truncated.starts_with("HEAD:"));
/// assert!(truncated.ends_with(":TAIL"));
/// assert!(truncated.contains("[Content truncated]"));
/// ```
#[must_use]
pub fn truncate_head_tail_utf16(text: &str, max_chars: usize, marker: &str) -> String {
    let total_units = utf8_len_to_utf16(text);
    if total_units <= max_chars {
        return text.to_owned();
    }
    if max_chars == 0 {
        return String::new();
    }

    let marker_units = utf8_len_to_utf16(marker);
    if max_chars <= marker_units {
        return utf16_slice_safe(text, 0, max_chars).to_owned();
    }

    let keep_units = max_chars - marker_units;
    let head_units = keep_units.div_ceil(2);
    let tail_units = keep_units / 2;

    let head = utf16_slice_safe(text, 0, head_units);
    let tail_start = total_units.saturating_sub(tail_units);
    let tail = utf16_slice_safe(text, tail_start, total_units);

    let mut out = String::with_capacity(head.len() + marker.len() + tail.len());
    out.push_str(head);
    out.push_str(marker);
    out.push_str(tail);
    out
}

/// Attachment classification used to deduct token allowances before handoff selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandoffAttachmentKind {
    /// Image attachment (`8,192` token allowance).
    Image,
    /// File or document attachment (`4,096` token allowance).
    File,
}

/// Computed character, byte, and token budgets for cross-provider thread handoffs.
///
/// # Examples
///
/// ```
/// use t3_providers::core::{DEFAULT_HANDOFF_TOKEN_CAP, HandoffAttachmentKind, HandoffBudget};
///
/// let budget = HandoffBudget::compute(
///     DEFAULT_HANDOFF_TOKEN_CAP,
///     64_000,
///     &[HandoffAttachmentKind::Image],
/// );
/// assert_eq!(budget.effective_token_cap, 16_000);
/// assert_eq!(budget.attachment_tokens, 8_192);
/// assert_eq!(budget.available_history_tokens, 7_808);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffBudget {
    /// Clamped total token ceiling.
    pub effective_token_cap: usize,
    /// Maximum serialized UTF-8 byte ceiling.
    pub byte_cap: usize,
    /// Tokens reserved for attachments.
    pub attachment_tokens: usize,
    /// Remaining tokens available for historical conversation turns.
    pub available_history_tokens: usize,
}

impl HandoffBudget {
    /// Computes remaining history capacity after clamping caps and deducting attachments.
    #[must_use]
    pub fn compute(
        requested_token_cap: usize,
        byte_cap: usize,
        attachments: &[HandoffAttachmentKind],
    ) -> Self {
        let effective_token_cap = clamp_handoff_token_cap(requested_token_cap);
        let attachment_tokens = attachments.iter().fold(0usize, |acc, kind| {
            let cost = match kind {
                HandoffAttachmentKind::Image => IMAGE_ATTACHMENT_TOKEN_ALLOWANCE,
                HandoffAttachmentKind::File => FILE_ATTACHMENT_TOKEN_ALLOWANCE,
            };
            acc.saturating_add(cost)
        });
        Self {
            effective_token_cap,
            byte_cap,
            attachment_tokens,
            available_history_tokens: effective_token_cap.saturating_sub(attachment_tokens),
        }
    }
}

/// Speaker role for a historical thread entry considered during handoff selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryRole {
    /// User message.
    User,
    /// Assistant response.
    Assistant,
    /// System/instruction message.
    System,
}

/// Historical conversation item considered by [`select_history`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// Stable message identifier.
    pub id: String,
    /// Speaker role.
    pub role: HistoryRole,
    /// Rendered textual content.
    pub text: String,
}

/// Output of [`select_history`], preserving chronological message ordering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedHistory {
    /// Selected entries in original chronological order.
    pub kept: Vec<HistoryEntry>,
    /// IDs of omitted entries (capped for omission-notice token bounds).
    pub omitted_ids: Vec<String>,
    /// Total count of omitted messages.
    pub omitted_total: usize,
    /// Estimated token cost of retained messages.
    pub used_tokens: usize,
    /// Total UTF-8 byte size of retained messages.
    pub used_bytes: usize,
}

/// Selects historical messages that fit within `budget`, prioritizing the last
/// user turn, last assistant turn, first user turn, and remaining newest turns.
#[must_use]
pub fn select_history(entries: &[HistoryEntry], budget: &HandoffBudget) -> SelectedHistory {
    if entries.is_empty() {
        return SelectedHistory {
            kept: Vec::new(),
            omitted_ids: Vec::new(),
            omitted_total: 0,
            used_tokens: 0,
            used_bytes: 0,
        };
    }

    let mut chosen = vec![false; entries.len()];
    let mut used_tokens = 0usize;
    let mut used_bytes = 0usize;

    let try_pick =
        |idx: usize, chosen: &mut [bool], used_tokens: &mut usize, used_bytes: &mut usize| {
            if chosen[idx] {
                return;
            }
            let tokens = estimate_text_tokens(&entries[idx].text).max(1);
            let bytes = entries[idx].text.len();
            if used_tokens.saturating_add(tokens) <= budget.available_history_tokens
                && used_bytes.saturating_add(bytes) <= budget.byte_cap
            {
                chosen[idx] = true;
                *used_tokens = used_tokens.saturating_add(tokens);
                *used_bytes = used_bytes.saturating_add(bytes);
            }
        };

    if let Some(last_user) = entries.iter().rposition(|e| e.role == HistoryRole::User) {
        try_pick(last_user, &mut chosen, &mut used_tokens, &mut used_bytes);
    }
    if let Some(last_asst) = entries
        .iter()
        .rposition(|e| e.role == HistoryRole::Assistant)
    {
        try_pick(last_asst, &mut chosen, &mut used_tokens, &mut used_bytes);
    }
    if let Some(first_user) = entries.iter().position(|e| e.role == HistoryRole::User) {
        try_pick(first_user, &mut chosen, &mut used_tokens, &mut used_bytes);
    }

    for idx in (0..entries.len()).rev() {
        try_pick(idx, &mut chosen, &mut used_tokens, &mut used_bytes);
    }

    let omission_token_cap = budget.effective_token_cap / 8;
    let mut omitted_tokens = 0usize;
    let mut kept = Vec::new();
    let mut omitted_ids = Vec::new();
    let mut omitted_total = 0usize;

    for (idx, entry) in entries.iter().enumerate() {
        if chosen[idx] {
            kept.push(entry.clone());
        } else {
            omitted_total += 1;
            let id_cost = estimate_text_tokens(&entry.id).saturating_add(1);
            if omitted_ids.len() < MAX_OMITTED_IDS_LISTED
                && omitted_tokens.saturating_add(id_cost) <= omission_token_cap
            {
                omitted_ids.push(entry.id.clone());
                omitted_tokens = omitted_tokens.saturating_add(id_cost);
            }
        }
    }

    SelectedHistory {
        kept,
        omitted_ids,
        omitted_total,
        used_tokens,
        used_bytes,
    }
}
