//! Claude Code session lifecycle, stream thinking promotion, resume guards,
//! binary replacement checks, and Homebrew channel upgrade hints.

use std::collections::HashSet;
use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;

use crate::core::compatibility::Semver;

fn parse_claude_semver(raw: &str) -> Option<Semver> {
    let clean = raw
        .trim()
        .trim_start_matches('v')
        .split(['-', '+'])
        .next()?;
    if let Some(strict) = Semver::parse_strict(clean) {
        return Some(strict);
    }
    let mut parts = clean.split('.');
    let major = parts.next()?.parse::<u64>().ok()?;
    let minor = parts.next().unwrap_or("0").parse::<u64>().ok()?;
    let patch = parts.next().unwrap_or("0").parse::<u64>().ok()?;
    parts.next().is_none().then_some(Semver {
        major,
        minor,
        patch,
    })
}

/// Homebrew stable cask command replaced when switching to `claude-code@latest` (`#39966`).
pub const CLAUDE_HOMEBREW_STABLE_UPDATE_COMMAND: &str = "brew upgrade --cask claude-code";

/// Homebrew switch command when stable cask trails a model's minimum version (`#39966`).
pub const CLAUDE_HOMEBREW_LATEST_SWITCH_COMMAND: &str =
    "brew uninstall --cask claude-code && brew install --cask claude-code@latest";

/// Active background task entry queued or tracked on a Claude native thread (`#30214`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeTrackedBackgroundTask {
    /// Task identifier awaited by the thread.
    pub task_id: String,
    /// Task category (e.g., shell command or subagent).
    pub kind: String,
    /// Short user-visible summary label.
    pub description: Option<String>,
}

/// Resolution plan emitted when a Claude query exits unexpectedly (`#30214`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeLostBackgroundWorkPlan {
    /// Background tasks whose outcome never reached Claude before process exit.
    pub lost_tasks: Vec<ClaudeTrackedBackgroundTask>,
    /// Whether `background_work.lost` + `provider_exit` wake continuation should be emitted.
    pub should_wake_agent: bool,
}

/// Collects background work lost when Claude exits unexpectedly without delivering outcomes (`#30214`).
///
/// Tasks whose completion is already buffered AND whose wake was explicitly requested
/// (`wake_requested`) are excluded because that requested continuation still delivers them.
#[must_use]
pub fn resolve_claude_lost_background_work<S: ::std::hash::BuildHasher>(
    exited_unexpectedly: bool,
    wake_requested: bool,
    buffered_task_notification_ids: &HashSet<String, S>,
    awaited_roster_tasks: &[ClaudeTrackedBackgroundTask],
    owed_wake_tasks: &[ClaudeTrackedBackgroundTask],
) -> ClaudeLostBackgroundWorkPlan {
    if !exited_unexpectedly {
        return ClaudeLostBackgroundWorkPlan {
            lost_tasks: Vec::new(),
            should_wake_agent: false,
        };
    }

    let mut seen = HashSet::<String>::new();
    let mut lost_tasks = Vec::new();
    for task in awaited_roster_tasks.iter().chain(owed_wake_tasks.iter()) {
        if wake_requested && buffered_task_notification_ids.contains(&task.task_id) {
            continue;
        }
        if seen.insert(task.task_id.clone()) {
            lost_tasks.push(task.clone());
        }
    }

    let should_wake_agent = !lost_tasks.is_empty();
    ClaudeLostBackgroundWorkPlan {
        lost_tasks,
        should_wake_agent,
    }
}

fn claude_model_version_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let compiled = Regex::new(r"claude-[a-z]+-(\d+)(?:-(\d+))?");
        // INFALLIBLE: constant Claude model generation regex (`#41085`).
        compiled.expect("claude model version regex must compile")
    })
}

/// Returns `true` for Claude 5.1+ models that may write pre-tool updates or steer replies
/// inside consecutive `thinking` blocks (`#41085`).
#[must_use]
pub fn claude_writes_updates_as_thinking(model: &str) -> bool {
    let Some(caps) = claude_model_version_regex().captures(model) else {
        return false;
    };
    let Some(major) = caps.get(1).and_then(|m| m.as_str().parse::<u32>().ok()) else {
        return false;
    };
    let minor = caps
        .get(2)
        .filter(|m| m.as_str().len() <= 2)
        .and_then(|m| m.as_str().parse::<u32>().ok())
        .unwrap_or(0);
    major > 5 || (major == 5 && minor >= 1)
}

/// State of a completed Claude `thinking` content block kept across subsequent blocks (`#41085`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeCompletedThinkingBlock {
    /// Parent assistant message identifier.
    pub message_id: String,
    /// Stream item identifier.
    pub item_id: String,
    /// Thinking text (may be empty under default display).
    pub text: String,
    /// `true` when preceded by another `thinking` block on the same message.
    pub follows_thinking: bool,
}

/// Tracks Claude `thinking` blocks per message so empty default-display thinking blocks
/// still anchor a following pre-tool update or steer reply (`#41085`).
#[derive(Debug, Default, Clone)]
pub struct ClaudeThinkingTracker {
    last_thinking: Option<ClaudeCompletedThinkingBlock>,
}

impl ClaudeThinkingTracker {
    /// Creates an empty thinking tracker.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a completed `thinking` block, retaining empty blocks as sequence anchors (`#41085`).
    pub fn record_thinking(&mut self, message_id: &str, item_id: &str, completed_text: &str) {
        let follows_thinking = self
            .last_thinking
            .as_ref()
            .is_some_and(|prev| prev.message_id == message_id);
        self.last_thinking = Some(ClaudeCompletedThinkingBlock {
            message_id: message_id.to_owned(),
            item_id: item_id.to_owned(),
            text: completed_text.to_owned(),
            follows_thinking,
        });
    }

    /// Returns the non-empty thinking text to surface as a visible pre-tool line when
    /// a `tool_use` follows consecutive thinking on a Claude 5.1+ model (`#41085`).
    #[must_use]
    pub fn take_pre_tool_update(&mut self, model: &str, message_id: &str) -> Option<String> {
        let update = self.last_thinking.take()?;
        if !update.text.trim().is_empty()
            && update.message_id == message_id
            && update.follows_thinking
            && claude_writes_updates_as_thinking(model)
        {
            Some(update.text)
        } else {
            None
        }
    }

    /// Clears state at message boundaries.
    pub fn clear(&mut self) {
        self.last_thinking = None;
    }
}

/// Minimal persisted Claude session history entry checked on resume (`#41120`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeHistoryEntry {
    /// Message role/type (`"user"`, `"assistant"`, etc.).
    pub message_type: String,
    /// Model tag when present (`"<synthetic>"` marks placeholder replies for failed requests).
    pub model: Option<String>,
}

/// Returns `true` when `messages` ends on the synthetic placeholder reply Claude writes
/// for a failed request (`#41120`).
#[must_use]
pub fn claude_history_ends_on_failed_request(messages: &[ClaudeHistoryEntry]) -> bool {
    messages.last().is_some_and(|last| {
        last.message_type == "assistant" && last.model.as_deref() == Some("<synthetic>")
    })
}

/// Returns the `CLAUDE_CODE_RESUME_*` env variables to inject on resume, omitting them when
/// history ends on a failed `<synthetic>` request so resume runs one turn instead of triggering
/// a phantom wake (`#41120`).
#[must_use]
pub fn claude_resume_interrupted_turn_env(
    is_resume: bool,
    history: &[ClaudeHistoryEntry],
) -> Option<[(&'static str, &'static str); 2]> {
    if !is_resume || claude_history_ends_on_failed_request(history) {
        return None;
    }
    Some([
        ("CLAUDE_CODE_RESUME_INTERRUPTED_TURN", "1"),
        ("CLAUDE_CODE_RESUME_REASON", "user_resume"),
    ])
}

/// Reports whether an update replaced the binary symlink target a live Claude process started from (`#41086`).
#[must_use]
pub fn claude_program_was_replaced(
    started_from: Option<&Path>,
    current_resolved: Option<&Path>,
) -> bool {
    matches!(
        (started_from, current_resolved),
        (Some(before), Some(now)) if before != now
    )
}

/// Compatibility state of an open Claude query when switching models (`#41086`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaudeLiveQueryReuse {
    /// No matching open query exists or the existing query is already stopping.
    AbsentOrStopping,
    /// An open query exists for the native thread, but its policy key differs.
    PolicyMismatch,
    /// A matching open query is active with the same policy key.
    MatchingActive,
}

/// Inputs evaluated when deciding whether switching models should restart a reused Claude CLI process (`#41086`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeModelSwitchRestartCheck<'a> {
    /// True if resuming rather than continuing a live session.
    pub is_resume: bool,
    /// Whether a live non-stopping query with a matching policy is open on the native thread.
    pub live_query: ClaudeLiveQueryReuse,
    /// API model ID used to start the live query.
    pub started_api_model_id: &'a str,
    /// New target API model ID.
    pub next_api_model_id: &'a str,
    /// Canonical path recorded when opening the live query.
    pub started_program: Option<&'a Path>,
    /// Current resolved canonical path on disk.
    pub current_program: Option<&'a Path>,
    /// True if background tasks or subagents are still running on the live query.
    pub has_running_background_work: bool,
}

/// Returns `true` when switching models should close the older Claude CLI and start the
/// updated binary instead of reusing the in-flight query (`#41086`).
#[must_use]
pub fn should_start_updated_claude_on_model_switch(
    input: &ClaudeModelSwitchRestartCheck<'_>,
) -> bool {
    !input.is_resume
        && input.live_query == ClaudeLiveQueryReuse::MatchingActive
        && input.started_api_model_id != input.next_api_model_id
        && claude_program_was_replaced(input.started_program, input.current_program)
        && !input.has_running_background_work
}

/// Formats a Claude Code version upgrade message scoped to the install channel's available
/// newest version, including Homebrew stable cask switch instructions when relevant (`#39966`).
#[must_use]
pub fn format_claude_version_upgrade_message(
    model_name: &str,
    min_version: &str,
    installed_version: Option<&str>,
    channel_latest_version: Option<&str>,
    channel_update_command: Option<&str>,
) -> String {
    let version_label = match installed_version {
        Some(v) if !v.trim().is_empty() => format!("v{}", v.trim()),
        _ => "the installed version".to_owned(),
    };
    if let Some(latest) = channel_latest_version
        && let (Some(latest_ver), Some(min_ver)) = (
            parse_claude_semver(latest),
            parse_claude_semver(min_version),
        )
        && latest_ver < min_ver
    {
        let too_old = format!(
            "Claude Code {version_label} is too old for {model_name}, which needs v{min_version} or newer"
        );
        if channel_update_command == Some(CLAUDE_HOMEBREW_STABLE_UPDATE_COMMAND) {
            return format!(
                "{too_old}, but the claude-code Homebrew cask only has v{latest}. Switch to the claude-code@latest cask: {CLAUDE_HOMEBREW_LATEST_SWITCH_COMMAND}"
            );
        }
        return format!(
            "{too_old}, but the newest version this install can update to is v{latest}. Install Claude Code another way or wait for a newer release."
        );
    }
    format!(
        "Claude Code {version_label} is too old for {model_name}. Upgrade to v{min_version} or newer to access it."
    )
}
