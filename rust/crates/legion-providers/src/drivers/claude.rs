//! Claude Code text generation fallback, gateway rate-limit cooldowns,
//! plugin-dir skill root resolution, truncation, and stream-json helpers.

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};

use legion_shared::utf16::{utf8_len_to_utf16, utf16_slice_safe};
use regex::Regex;

use super::tokenize_cli_args;

/// Claude Code active steering queues tools cleanly instead of interrupting running tools (`be9176761c`).
pub const CLAUDE_ACTIVE_STEERING_INTERRUPTS_TOOLS: bool = false;

/// Claude CLI flags that consume the immediately following argument token even when it begins
/// with `--` (`a1b9e1cc87`).
pub const CLAUDE_LAUNCH_VALUE_FLAGS: &[&str] = &["append-system-prompt"];

/// Error message surfaced when a disabled provider is selected for text generation (`6f5293fbdc`).
pub const NO_TEXT_GENERATION_PROVIDER_MESSAGE: &str = "No text generation provider is available. Enable a provider that can generate text in Settings \u{2192} Providers.";

const NO_SESSION_PERSISTENCE_REJECTED: &str = "error: unknown option '--no-session-persistence'";

fn images_unsupported_regex() -> Option<&'static Regex> {
    static RE: OnceLock<Option<Regex>> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)image[^\n]*(?:not support|unsupported)|(?:not support|unsupported)[^\n]*image",
        )
        .ok()
    })
    .as_ref()
}

/// Returns `true` when Claude CLI output indicates the target model/endpoint rejects images (`6f82dd9e79`).
#[must_use]
pub fn is_claude_images_unsupported_error(stdout: &str, stderr: &str) -> bool {
    let combined = format!("{stdout}\n{stderr}");
    images_unsupported_regex().is_some_and(|re| re.is_match(&combined))
}

/// Thread-safe memoization of Claude/Anthropic-compatible model IDs that reject images (`6f82dd9e79`).
#[derive(Debug, Default)]
pub struct ClaudeTextOnlyModelsCache {
    models: Mutex<BTreeSet<String>>,
}

impl ClaudeTextOnlyModelsCache {
    /// Creates an empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `true` when `api_model_id` was previously confirmed text-only.
    #[must_use]
    pub fn is_text_only(&self, api_model_id: &str) -> bool {
        let guard = self.models.lock().unwrap_or_else(PoisonError::into_inner);
        guard.contains(api_model_id)
    }

    /// Records `api_model_id` if an image attempt failed with `IMAGES_UNSUPPORTED`
    /// and a text-only retry succeeded (`6f82dd9e79`).
    pub fn record_if_unsupported_then_succeeded(
        &self,
        api_model_id: &str,
        image_stdout: &str,
        image_stderr: &str,
        text_only_succeeded: bool,
    ) -> bool {
        if !text_only_succeeded || !is_claude_images_unsupported_error(image_stdout, image_stderr) {
            return false;
        }
        let mut guard = self.models.lock().unwrap_or_else(PoisonError::into_inner);
        guard.insert(api_model_id.to_owned());
        true
    }
}

/// Decision for Claude CLI text-generation retry sequencing (`7fcec368cb`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaudeTextGenRetryDecision {
    /// Retry the same image mode without `--no-session-persistence`.
    RetryWithoutNoSessionPersistence,
    /// Strip image blocks and retry text-only (`7fcec368cb`).
    RetryWithoutImages,
    /// No retry applies; accept or fail permanently.
    DoNotRetry,
}

/// Evaluates whether a failed Claude CLI text-generation attempt should retry without
/// `--no-session-persistence` or without image blocks (`7fcec368cb`).
#[must_use]
pub fn classify_claude_text_gen_retry(
    exit_code: i32,
    stderr: &str,
    with_images: bool,
    used_no_session_persistence: bool,
) -> ClaudeTextGenRetryDecision {
    if exit_code == 0 {
        return ClaudeTextGenRetryDecision::DoNotRetry;
    }
    if used_no_session_persistence && stderr.contains(NO_SESSION_PERSISTENCE_REJECTED) {
        return ClaudeTextGenRetryDecision::RetryWithoutNoSessionPersistence;
    }
    if with_images {
        return ClaudeTextGenRetryDecision::RetryWithoutImages;
    }
    ClaudeTextGenRetryDecision::DoNotRetry
}

/// Resolves an aggregate rate-limit cooldown reset timestamp while dropping placeholder
/// `"unknown"` entries that have `reset_at == None` once a gateway 429 result supplies
/// an explicit reset timestamp (`e3294d7663`).
#[must_use]
pub fn claude_gateway_cooldown_reset_at(
    tracked_limits: &[(Option<&str>, Option<i64>)],
    assistant_limit: Option<(Option<&str>, Option<i64>)>,
    result_reset_at: Option<i64>,
) -> Option<i64> {
    let mut times: Vec<Option<i64>> = tracked_limits
        .iter()
        .filter(|(limit_type, time)| {
            !(*limit_type == Some("unknown") && time.is_none() && result_reset_at.is_some())
        })
        .map(|(_, time)| *time)
        .collect();
    if let Some((Some(_), reset_at)) = assistant_limit {
        times.push(reset_at);
    }
    if let Some(reset_at) = result_reset_at {
        times.push(Some(reset_at));
    }
    if times.is_empty() || times.iter().any(Option::is_none) {
        return None;
    }
    times.into_iter().flatten().max()
}

/// Phase attached to held assistant text when Claude reaches a native turn boundary
/// right before a late-read steer continues the run (`0b02be3d5f`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaudeAssistantPhase {
    /// Held reply completes visibly with `phase: "final_answer"` while steer continues.
    FinalAnswer,
}

/// Determines whether a held Claude `result` with echoed steer prompt UUIDs should
/// publish the latest assistant reply immediately as `FinalAnswer` (`0b02be3d5f`).
#[must_use]
pub const fn phase_for_held_steer_result(
    has_echoed_prompt_uuids: bool,
    has_last_assistant_reply: bool,
) -> Option<ClaudeAssistantPhase> {
    if has_echoed_prompt_uuids && has_last_assistant_reply {
        Some(ClaudeAssistantPhase::FinalAnswer)
    } else {
        None
    }
}

/// Extracts repeatable `--plugin-dir` values from `launch_args`, honoring [`CLAUDE_LAUNCH_VALUE_FLAGS`]
/// so flags such as `--append-system-prompt --plugin-dir` never misparse (`a1b9e1cc87`).
#[must_use]
pub fn claude_launch_arg_plugin_directories(launch_args: Option<&str>) -> Vec<String> {
    let Some(raw) = launch_args else {
        return Vec::new();
    };
    let tokens = tokenize_cli_args(raw);
    let mut directories = Vec::new();
    let mut i = 0;
    while i < tokens.len() && tokens[i] != "--" {
        let token = &tokens[i];
        if !token.starts_with("--") || token.contains('=') {
            if let Some(val) = token.strip_prefix("--plugin-dir=") {
                directories.push(val.to_owned());
            }
            i += 1;
            continue;
        }
        let Some(next) = tokens.get(i + 1) else {
            i += 1;
            continue;
        };
        let flag_name = &token[2..];
        let takes_next = !next.starts_with("--") || CLAUDE_LAUNCH_VALUE_FLAGS.contains(&flag_name);
        if !takes_next {
            i += 1;
            continue;
        }
        if token == "--plugin-dir" {
            directories.push(next.clone());
        }
        i += 2;
    }
    directories
}

/// Resolves ordered deduplicated inline plugin directories from CLI `--plugin-dir` flags
/// (resolved relative to `cwd`, without `~/` expansion) followed by `CLAUDE_CODE_PLUGIN_DIRS`
/// entries (expanding `~/` and keeping only absolute paths) (`a1b9e1cc87`).
#[must_use]
pub fn resolve_claude_inline_plugin_directories(
    launch_arg_dirs: &[String],
    env_plugin_dirs: Option<&str>,
    cwd: Option<&Path>,
    home: &Path,
    path_list_separator: char,
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();

    for entry in launch_arg_dirs {
        if entry.is_empty() {
            continue;
        }
        let path = Path::new(entry);
        let resolved = if path.is_absolute() {
            path.to_path_buf()
        } else if let Some(base) = cwd {
            base.join(path)
        } else {
            continue;
        };
        if seen.insert(resolved.clone()) {
            out.push(resolved);
        }
    }

    if let Some(raw_env) = env_plugin_dirs {
        for raw_entry in raw_env.split(path_list_separator) {
            let trimmed = raw_entry.trim();
            if trimmed.is_empty() {
                continue;
            }
            let expanded = expand_tilde(trimmed, home);
            if expanded.is_absolute() && seen.insert(expanded.clone()) {
                out.push(expanded);
            }
        }
    }

    out
}

fn expand_tilde(raw: &str, home: &Path) -> PathBuf {
    if raw == "~" {
        home.to_path_buf()
    } else if let Some(rest) = raw.strip_prefix("~/").or_else(|| raw.strip_prefix("~\\")) {
        home.join(rest)
    } else {
        PathBuf::from(raw)
    }
}

/// Returns `true` for Claude Fable 5.2 slugs/aliases configured with `fastMode` (`76bd3bb098`).
#[must_use]
pub fn claude_fable_supports_fast_mode(model_id: &str) -> bool {
    let lower = model_id.trim().to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "fable" | "fable-5.2" | "claude-fable-5-2" | "claude-fable-5.2"
    )
}

/// Truncates `text` to at most `max_utf16_units` without splitting a UTF-8 codepoint or
/// leaving an orphaned UTF-16 high surrogate (`c688453acd`).
#[must_use]
pub fn truncate_without_trailing_high_surrogate(text: &str, max_utf16_units: usize) -> &str {
    if utf8_len_to_utf16(text) <= max_utf16_units {
        return text;
    }
    utf16_slice_safe(text, 0, max_utf16_units)
}

/// Formats a bounded Claude tool request summary (`command`, `path`, or JSON fallback)
/// without splitting surrogate pairs or multi-byte sequences (`c688453acd`).
#[must_use]
pub fn summarize_claude_tool_request(
    tool_name: &str,
    command: Option<&str>,
    path: Option<&str>,
    serialized_json: &str,
) -> String {
    if let Some(cmd) = command {
        let clipped = truncate_without_trailing_high_surrogate(cmd, 400);
        return format!("{tool_name}: {clipped}");
    }
    if let Some(p) = path {
        let clipped = truncate_without_trailing_high_surrogate(p, 400);
        return format!("{tool_name}: {clipped}");
    }
    if utf8_len_to_utf16(serialized_json) <= 400 {
        format!("{tool_name}: {serialized_json}")
    } else {
        let clipped = truncate_without_trailing_high_surrogate(serialized_json, 397);
        format!("{tool_name}: {clipped}...")
    }
}

/// Validates that a selected provider is enabled and capable before running text generation (`6f5293fbdc`).
pub fn check_text_generation_provider_usable(
    enabled: Option<bool>,
    text_generation_capable: bool,
    has_requested_model: bool,
) -> Result<(), &'static str> {
    if enabled == Some(false) {
        return Err(NO_TEXT_GENERATION_PROVIDER_MESSAGE);
    }
    if !text_generation_capable || !has_requested_model {
        return Err(NO_TEXT_GENERATION_PROVIDER_MESSAGE);
    }
    Ok(())
}
