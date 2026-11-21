//! Value types and push-payload parser for localized Live Activity presentations (`BUG-24`).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Normalized phase of an agent activity row (`AgentActivityPresentation.kt`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentActivityPhase {
    /// Agent is starting / connecting.
    Starting,
    /// Agent is actively running.
    Running,
    /// Agent is blocked waiting for user approval.
    WaitingForApproval,
    /// Agent is blocked waiting for user input.
    WaitingForInput,
    /// Status update is stale / waiting.
    Stale,
    /// Agent turn completed cleanly.
    Completed,
    /// Agent turn failed.
    Failed,
}

impl AgentActivityPhase {
    /// Parses wire `activity_phase` strings or legacy status names.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "starting" | "Connecting" => Some(Self::Starting),
            "running" | "Working" => Some(Self::Running),
            "waiting_for_approval" | "Approval" => Some(Self::WaitingForApproval),
            "waiting_for_input" | "Input" => Some(Self::WaitingForInput),
            "stale" | "Waiting" => Some(Self::Stale),
            "completed" | "Done" => Some(Self::Completed),
            "failed" | "Failed" => Some(Self::Failed),
            _ => None,
        }
    }

    /// Returns `true` when user attention (approval or input) is required.
    pub const fn needs_user(self) -> bool {
        matches!(self, Self::WaitingForApproval | Self::WaitingForInput)
    }

    /// Returns `true` when the phase represents a terminal outcome.
    pub const fn is_finished(self) -> bool {
        matches!(self, Self::Completed | Self::Failed)
    }
}

/// Single row parsed from push `activity_line_{0..4}` data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentActivityRowInput {
    /// Phase of this individual row.
    pub phase: AgentActivityPhase,
    /// Thread title.
    pub title: String,
    /// Project title.
    pub project: String,
}

/// Input model mirroring Android `AgentActivityPresentation.kt` push payloads.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentActivityInput {
    /// Ordered rows (up to 5) from the relay.
    pub rows: Vec<AgentActivityRowInput>,
    /// Optional explicit aggregate phase.
    pub phase: Option<AgentActivityPhase>,
    /// Whether the Live Activity is currently active.
    pub active: bool,
    /// Optional explicit active agent count override.
    pub active_count: Option<usize>,
    /// Optional explicit attention count override.
    pub attention_count: Option<usize>,
    /// Optional fallback title when `rows` is empty.
    pub fallback_title: Option<String>,
    /// Optional fallback chip label when `phase` is `None`.
    pub fallback_chip: Option<String>,
}

impl AgentActivityInput {
    /// Parses a raw push `data` map (`activity_line_0..4`, `activity_phase`, etc.).
    pub fn from_push_data(data: &BTreeMap<String, String>, active: bool) -> Self {
        let mut rows = Vec::new();
        for idx in 0..5 {
            let key = format!("activity_line_{idx}");
            let Some(raw) = data.get(&key) else {
                continue;
            };
            let parts: Vec<&str> = raw.splitn(3, '\t').collect();
            if parts.len() == 3 && !parts[1].trim().is_empty() {
                let phase =
                    AgentActivityPhase::parse(parts[0]).unwrap_or(AgentActivityPhase::Stale);
                rows.push(AgentActivityRowInput {
                    phase,
                    title: truncate_chars(parts[1], 120),
                    project: truncate_chars(parts[2], 120),
                });
            }
        }
        let phase = data
            .get("activity_phase")
            .and_then(|raw| AgentActivityPhase::parse(raw))
            .or_else(|| rows.first().map(|r| r.phase));
        let active_count = data
            .get("activity_active_count")
            .and_then(|s| s.parse::<usize>().ok());
        let attention_count = data
            .get("activity_attention_count")
            .and_then(|s| s.parse::<usize>().ok());
        Self {
            rows,
            phase,
            active,
            active_count,
            attention_count,
            fallback_title: data
                .get("activity_title")
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty()),
            fallback_chip: data
                .get("activity_chip")
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty()),
        }
    }
}

/// Localized representation of a single Live Activity row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalizedAgentActivityRow {
    /// Phase enum value.
    pub phase: AgentActivityPhase,
    /// Localized status badge string (`"Working"`, `"Approval"`, ...).
    pub status_label: String,
    /// Thread title.
    pub title: String,
    /// Project title.
    pub project: String,
}

/// Complete localized Live Activity / ongoing notification presentation (`BUG-24`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentLiveActivityPresentation {
    /// Canonical `t3_edge::i18n` catalog locale code.
    pub locale: String,
    /// Primary notification summary title.
    pub summary: String,
    /// Optional multi-row subtext (`"<project> · <count>"`).
    pub subtext: Option<String>,
    /// Short Live Update / Dynamic Island chip text.
    pub chip: String,
    /// Primary localized action button (`"Open"`, `"Approve"`, `"Answer"`), or `None` when inactive.
    pub action: Option<String>,
    /// Localized `"Dismiss"` button label.
    pub dismiss_label: String,
    /// Localized heading for the primary phase, if resolved.
    pub phase_heading: Option<String>,
    /// Localized row entries preserving relay ordering.
    pub rows: Vec<LocalizedAgentActivityRow>,
}

pub(crate) fn truncate_chars(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}
