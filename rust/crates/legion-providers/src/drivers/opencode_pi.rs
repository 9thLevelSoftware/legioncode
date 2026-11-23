//! `OpenCode` reasoning variant labels, size-scoped legacy checks, custom default choices,
//! `OpenCode` HTTP usage-limit error surfacing, and Pi async subagent settlement guards.

use std::collections::HashMap;
use std::sync::OnceLock;

use regex::Regex;
use serde::Deserialize;

/// Choice sentinel emitted for custom/unrecognized `OpenCode` models so users can return
/// to sending no explicit `variant` parameter (`39480b5c88`).
pub const DEFAULT_VARIANT_CHOICE_ID: &str = "legioncode:default";

const OPENCODE_PROBE_FAILED_MESSAGE: &str = "OpenCode could not read workspace usage.";

/// Formats an `OpenCode` / Pi reasoning variant identifier for display (`4d5bf2d557`).
///
/// Specifically labels `"xhigh"` as `"Extra High"` instead of `"Xhigh"`.
#[must_use]
pub fn reasoning_variant_label(id: &str) -> String {
    match id {
        "xhigh" => "Extra High".to_owned(),
        "high" => "High".to_owned(),
        "medium" => "Medium".to_owned(),
        "low" => "Low".to_owned(),
        "minimal" => "Minimal".to_owned(),
        "none" => "None".to_owned(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(first) => {
                    let mut out = first.to_uppercase().to_string();
                    out.push_str(chars.as_str());
                    out
                }
                None => String::new(),
            }
        }
    }
}

/// Option choice inside an `OpenCode` model's `"variant"` select descriptor (`39480b5c88`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenCodeVariantOption {
    /// Wire option ID (`"low"`, `"xhigh"`, or [`DEFAULT_VARIANT_CHOICE_ID`]).
    pub id: String,
    /// User-facing option label.
    pub label: String,
    /// Marked `true` only when inferred default matches this option.
    pub is_default: bool,
}

/// `OpenCode` `"variant"` reasoning select descriptor (`39480b5c88`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenCodeVariantDescriptor {
    /// Available reasoning choices (`legioncode:default` prepended when default is unknown).
    pub options: Vec<OpenCodeVariantOption>,
    /// Selected default choice ID (`inferred` or `legioncode:default`).
    pub current_value: String,
}

/// Builds `OpenCode`'s `"variant"` descriptor, adding a `"Default"` (`legioncode:default`) option
/// when no model default can be inferred so custom models can return to server default (`39480b5c88`).
#[must_use]
pub fn open_code_variant_descriptor(
    inferred_default: Option<&str>,
    variants: &[&str],
) -> Option<OpenCodeVariantDescriptor> {
    if variants.is_empty() {
        return None;
    }
    let mut options: Vec<OpenCodeVariantOption> = variants
        .iter()
        .map(|id| OpenCodeVariantOption {
            id: (*id).to_owned(),
            label: reasoning_variant_label(id),
            is_default: inferred_default == Some(*id),
        })
        .collect();
    if inferred_default.is_none() {
        options.insert(
            0,
            OpenCodeVariantOption {
                id: DEFAULT_VARIANT_CHOICE_ID.to_owned(),
                label: "Default".to_owned(),
                is_default: false,
            },
        );
    }
    Some(OpenCodeVariantDescriptor {
        options,
        current_value: inferred_default
            .unwrap_or(DEFAULT_VARIANT_CHOICE_ID)
            .to_owned(),
    })
}

/// Strips [`DEFAULT_VARIANT_CHOICE_ID`] back to `None` before sending requests to `OpenCode` (`39480b5c88`).
#[must_use]
pub fn open_code_wire_variant(selected: Option<&str>) -> Option<&str> {
    selected.filter(|value| *value != DEFAULT_VARIANT_CHOICE_ID && !value.trim().is_empty())
}

fn param_size_token_regex() -> Option<&'static Regex> {
    static RE: OnceLock<Option<Regex>> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\d+(?:\.\d+)?[bm]$").ok())
        .as_ref()
}

/// Formats base `(input, output)` token prices used to identify renamed same-size models (`da5bf96303`).
#[must_use]
pub fn open_code_price(input: f64, output: f64) -> String {
    format!("{input}/{output}")
}

/// Derives `OpenCode`'s size-scoped release line by preserving parameter-size tokens such as
/// `8b` or `70b` while stripping version digits from other tokens (`da5bf96303`).
#[must_use]
pub fn open_code_release_line(id: &str) -> String {
    let size_re = param_size_token_regex();
    id.to_ascii_lowercase()
        .split(['-', '_'])
        .map(|token| {
            if size_re.is_some_and(|re| re.is_match(token)) {
                token.to_owned()
            } else {
                token
                    .chars()
                    .filter(|ch| !ch.is_ascii_digit() && *ch != '.')
                    .collect()
            }
        })
        .collect::<String>()
}

/// Release metadata for one `OpenCode` catalog model entry (`da5bf96303`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenCodeReleaseMeta {
    pub slug: String,
    pub provider_id: String,
    pub model_id: String,
    pub family: Option<String>,
    pub released_ms: Option<i64>,
    pub deprecated: bool,
    pub price: Option<String>,
}

/// Evaluates each `OpenCode` catalog entry and returns `(slug, is_legacy)` using size-scoped
/// and price-matched supersession rules within `(provider_id, family)` (`da5bf96303`).
#[must_use]
pub fn mark_open_code_legacy(releases: &[OpenCodeReleaseMeta]) -> Vec<(String, bool)> {
    struct ActiveEntry {
        released_ms: i64,
        line: String,
        price: Option<String>,
    }

    let mut active_by_family: HashMap<String, Vec<ActiveEntry>> = HashMap::new();
    for r in releases {
        let Some(family) = r.family.as_deref().filter(|f| !f.is_empty()) else {
            continue;
        };
        let Some(released_ms) = r.released_ms else {
            continue;
        };
        if r.deprecated {
            continue;
        }
        let key = format!("{}/{}", r.provider_id, family);
        active_by_family.entry(key).or_default().push(ActiveEntry {
            released_ms,
            line: open_code_release_line(&r.model_id),
            price: r.price.clone(),
        });
    }

    releases
        .iter()
        .map(|r| {
            let line = open_code_release_line(&r.model_id);
            let superseded = r
                .family
                .as_deref()
                .filter(|f| !f.is_empty())
                .and_then(|family| {
                    let released_ms = r.released_ms?;
                    let key = format!("{}/{}", r.provider_id, family);
                    active_by_family.get(&key).map(|others| {
                        others.iter().any(|other| {
                            other.released_ms > released_ms
                                && (other.line == line
                                    || (r.price.is_some() && other.price == r.price))
                        })
                    })
                })
                .unwrap_or(false);
            (r.slug.clone(), r.deprecated || superseded)
        })
        .collect()
}

#[derive(Deserialize)]
struct OpenCodeErrorBody {
    message: String,
}

/// Extracts `OpenCode`'s non-2xx JSON `message` field for usage-limit probe failures (`9c25b71395`).
#[must_use]
pub fn opencode_usage_error_message(http_status: u16, response_body: &str) -> String {
    if (200..300).contains(&http_status) {
        return String::new();
    }
    serde_json::from_str::<OpenCodeErrorBody>(response_body)
        .ok()
        .map(|parsed| parsed.message.trim().to_owned())
        .filter(|msg| !msg.is_empty())
        .unwrap_or_else(|| OPENCODE_PROBE_FAILED_MESSAGE.to_owned())
}

/// Async child step entry reported inside a `pi-subagents` run snapshot (`c53ac237a0`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PiAsyncChildStep<'a> {
    pub id: Option<&'a str>,
    pub kind: Option<&'a str>,
    pub agent: Option<&'a str>,
    pub state: Option<&'a str>,
}

fn pi_async_status_is_active(state: Option<&str>) -> Option<bool> {
    match state?.trim().to_ascii_lowercase().as_str() {
        "running" | "pending" | "queued" => Some(true),
        "complete" | "completed" | "done" | "failed" | "error" | "cancelled" | "stopped" => {
            Some(false)
        }
        _ => None,
    }
}

/// Returns `true` only when both the async parent run state AND all valid child `"step"`
/// entries have settled out of `running` / `pending` (`c53ac237a0`).
#[must_use]
pub fn pi_async_run_should_settle(
    run_state: Option<&str>,
    children: &[PiAsyncChildStep<'_>],
) -> bool {
    let Some(run_active) = pi_async_status_is_active(run_state) else {
        return false;
    };
    if run_active {
        return false;
    }
    for child in children {
        if child.kind != Some("step") || child.id.is_none() || child.agent.is_none() {
            continue;
        }
        if pi_async_status_is_active(child.state) == Some(true) {
            return false;
        }
    }
    true
}

/// Result after sending an `OpenCode` `session.command` when checking whether the command triggered
/// an actual model execution or finished locally without prompting (`#27378`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenCodeCommandCompletionStatus {
    /// Leave execution open because session is busy or history shows a post-boundary message.
    WaitForExecutionEvents,
    /// Complete the turn immediately because no execution started.
    CompleteImmediatelyCompleted,
    /// Complete the turn as interrupted because caller interrupted before idle settle.
    CompleteImmediatelyInterrupted,
}

/// Observed session execution activity right after `OpenCode` `session.command` returns (`#27378`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenCodeSessionActivity {
    /// Session is idle, absent from active turn tracking, and has no post-boundary activity messages.
    IdleNoActivity,
    /// Session status is still `busy` or `retry`.
    Busy,
    /// Session remains registered in active turn tracking.
    TrackedInActiveMap,
    /// Session message history contains messages created after the pre-command boundary.
    PostBoundaryMessageObserved,
}

/// Evaluates whether an `OpenCode` slash command that never prompted the model should immediately
/// finalize its turn (`#27378`).
#[must_use]
pub const fn opencode_should_complete_command_immediately(
    is_still_active_turn: bool,
    activity: OpenCodeSessionActivity,
    turn_interrupted: bool,
) -> OpenCodeCommandCompletionStatus {
    if !is_still_active_turn || !matches!(activity, OpenCodeSessionActivity::IdleNoActivity) {
        return OpenCodeCommandCompletionStatus::WaitForExecutionEvents;
    }
    if turn_interrupted {
        OpenCodeCommandCompletionStatus::CompleteImmediatelyInterrupted
    } else {
        OpenCodeCommandCompletionStatus::CompleteImmediatelyCompleted
    }
}
