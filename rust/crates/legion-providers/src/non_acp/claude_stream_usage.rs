//! Stateful Claude `stream-json` usage accumulation tolerating empty `{}` or `null`
//! stream usage frames (`#40804`).

use serde_json::Value;

/// Accumulates nullable/partial Claude stream usage across `message_start`, `message_delta`,
/// and `assistant` snapshot frames without stalling on `{}` or `null` token fields (`#40804`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClaudeStreamUsage {
    /// Prompt tokens not served from cache.
    pub input_tokens: u64,
    /// Prompt tokens written to cache.
    pub cache_creation_input_tokens: Option<u64>,
    /// Prompt tokens read from cache.
    pub cache_read_input_tokens: Option<u64>,
    /// Output tokens generated so far.
    pub output_tokens: u64,
}

impl ClaudeStreamUsage {
    /// Initializes stream usage from `message_start.message.usage` (defaulting `{}`/`null` cleanly).
    #[must_use]
    pub fn from_message_start(usage: Option<&Value>) -> Self {
        Self {
            input_tokens: optional_claude_token(usage, "input_tokens").unwrap_or(0),
            cache_creation_input_tokens: optional_claude_token(
                usage,
                "cache_creation_input_tokens",
            ),
            cache_read_input_tokens: optional_claude_token(usage, "cache_read_input_tokens"),
            output_tokens: optional_claude_token(usage, "output_tokens").unwrap_or(0),
        }
    }

    /// Merges partial `message_delta.usage` into current stream totals, retaining prior non-null counts.
    #[must_use]
    pub fn merge_delta(&self, delta: Option<&Value>) -> Self {
        Self {
            input_tokens: optional_claude_token(delta, "input_tokens").unwrap_or(self.input_tokens),
            cache_creation_input_tokens: optional_claude_token(
                delta,
                "cache_creation_input_tokens",
            )
            .or(self.cache_creation_input_tokens),
            cache_read_input_tokens: optional_claude_token(delta, "cache_read_input_tokens")
                .or(self.cache_read_input_tokens),
            output_tokens: optional_claude_token(delta, "output_tokens")
                .unwrap_or(self.output_tokens),
        }
    }

    /// Combines an `assistant` message snapshot using `max(output_tokens)` (`#40804`).
    #[must_use]
    pub fn merge_assistant_snapshot(&self, snapshot: Option<&Value>) -> Self {
        let mut next = Self::from_message_start(snapshot);
        next.output_tokens = next.output_tokens.max(self.output_tokens);
        next
    }

    /// Total prompt tokens including cache creation and cache reads.
    #[must_use]
    pub const fn total_input_tokens(&self) -> u64 {
        self.input_tokens
            .saturating_add(match self.cache_creation_input_tokens {
                Some(n) => n,
                None => 0,
            })
            .saturating_add(match self.cache_read_input_tokens {
                Some(n) => n,
                None => 0,
            })
    }
}

/// Parses an optional non-negative finite token count from `usage[key]`, returning `None` for
/// `null`, negative, or non-finite values (`#40804`).
#[must_use]
pub fn optional_claude_token(usage: Option<&Value>, key: &str) -> Option<u64> {
    let val = usage.and_then(|u| u.get(key))?;
    if let Some(n) = val.as_u64() {
        return Some(n);
    }
    let f = val.as_f64()?;
    if f.is_finite() && f >= 0.0 {
        let rounded = f.round();
        // Compare in string space only after formatting integer range.
        (rounded <= 1.844_674_407_370_955e19)
            .then(|| format!("{rounded:.0}").parse::<u64>().ok())
            .flatten()
    } else {
        None
    }
}
