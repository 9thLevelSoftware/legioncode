//! Child environment sanitization for `BUG-09` process spawning.
//!
//! Strips Node watch-mode coordination variables (`WATCH_REPORT_DEPENDENCIES`,
//! `NODE_WATCH_*`) and inherited inspector/debugger flags (`--inspect`,
//! `--inspect-brk`, `--inspect-port=...`, `--inspect-wait`, `--inspect-publish-uid`)
//! from `NODE_OPTIONS` before spawning worker or agent child processes.

use std::collections::BTreeMap;
use std::process::Command;

/// Environment variable used by `node --watch` to hijack worker/fork IPC channels.
pub const WATCH_REPORT_DEPENDENCIES_KEY: &str = "WATCH_REPORT_DEPENDENCIES";

/// Environment prefix used by `node --watch` child coordination variables.
pub const NODE_WATCH_ENV_PREFIX: &str = "NODE_WATCH_";

/// Node runtime options variable scrubbed of inherited debugger/inspector flags.
pub const NODE_OPTIONS_KEY: &str = "NODE_OPTIONS";

/// Returns `true` when `key` is a watch-mode variable that must never leak to child processes.
pub fn is_blocked_child_env_key(key: &str) -> bool {
    let trimmed = key.trim();
    if trimmed.is_empty() {
        return true;
    }
    let upper = trimmed.to_ascii_uppercase();
    upper == WATCH_REPORT_DEPENDENCIES_KEY || upper.starts_with(NODE_WATCH_ENV_PREFIX)
}

fn is_node_options_key(key: &str, is_windows: bool) -> bool {
    if is_windows {
        key.eq_ignore_ascii_case(NODE_OPTIONS_KEY)
    } else {
        key == NODE_OPTIONS_KEY
    }
}

#[derive(Debug)]
struct OptionToken<'a> {
    raw: &'a str,
    unquoted: String,
}

fn tokenize_node_options(input: &str) -> Vec<OptionToken<'_>> {
    let bytes = input.as_bytes();
    let mut tokens = Vec::new();
    let mut idx = 0usize;

    while idx < bytes.len() {
        while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
            idx += 1;
        }
        if idx >= bytes.len() {
            break;
        }

        let start = idx;
        let mut unquoted = String::new();
        let mut quote: Option<u8> = None;

        while idx < bytes.len() {
            let b = bytes[idx];
            if let Some(active_quote) = quote {
                if b == active_quote {
                    quote = None;
                    idx += 1;
                    continue;
                }
                if active_quote == b'"'
                    && b == b'\\'
                    && let Some(&next) = bytes.get(idx + 1)
                {
                    unquoted.push(char::from(next));
                    idx += 2;
                    continue;
                }
                let ch = input[idx..].chars().next().unwrap_or('\u{FFFD}');
                unquoted.push(ch);
                idx += ch.len_utf8();
                continue;
            }

            if b.is_ascii_whitespace() {
                break;
            }
            if b == b'"' || b == b'\'' {
                quote = Some(b);
                idx += 1;
                continue;
            }
            if b == b'\\'
                && let Some(&next) = bytes.get(idx + 1)
            {
                unquoted.push(char::from(next));
                idx += 2;
                continue;
            }
            let ch = input[idx..].chars().next().unwrap_or('\u{FFFD}');
            unquoted.push(ch);
            idx += ch.len_utf8();
        }

        tokens.push(OptionToken {
            raw: &input[start..idx],
            unquoted,
        });
    }

    tokens
}

fn is_debugger_flag_without_arg(flag: &str) -> bool {
    matches!(flag, "--inspect" | "--inspect-brk" | "--inspect-wait")
        || flag.starts_with("--inspect=")
        || flag.starts_with("--inspect-brk=")
        || flag.starts_with("--inspect-port=")
        || flag.starts_with("--inspect-wait=")
        || flag.starts_with("--inspect-publish-uid=")
}

fn takes_separate_debugger_value(flag: &str) -> bool {
    matches!(flag, "--inspect-port" | "--inspect-publish-uid")
}

/// Removes Node inspector/debugger flags from a `NODE_OPTIONS` string while keeping all other flags.
///
/// Returns `None` when no non-debugger options remain.
///
/// # Examples
///
/// ```
/// use t3_process_ffi::env::sanitize_node_options;
///
/// assert_eq!(
///     sanitize_node_options("--max-old-space-size=4096 --inspect-brk --inspect-port=9229"),
///     Some("--max-old-space-size=4096".to_owned()),
/// );
/// assert_eq!(sanitize_node_options("--inspect"), None);
/// ```
pub fn sanitize_node_options(raw: &str) -> Option<String> {
    let tokens = tokenize_node_options(raw);
    let mut kept = Vec::with_capacity(tokens.len());
    let mut idx = 0usize;

    while idx < tokens.len() {
        let flag = tokens[idx].unquoted.as_str();
        if is_debugger_flag_without_arg(flag) {
            idx += 1;
            continue;
        }
        if takes_separate_debugger_value(flag) {
            idx += 1;
            if idx < tokens.len() && !tokens[idx].unquoted.starts_with('-') {
                idx += 1;
            }
            continue;
        }
        kept.push(tokens[idx].raw);
        idx += 1;
    }

    if kept.is_empty() {
        None
    } else {
        Some(kept.join(" "))
    }
}

fn insert_case_aware(
    target: &mut BTreeMap<String, String>,
    key: String,
    value: String,
    is_windows: bool,
) {
    if !is_windows {
        target.insert(key, value);
        return;
    }

    let upper = key.to_ascii_uppercase();
    let stale: Vec<String> = target
        .keys()
        .filter(|existing| existing.to_ascii_uppercase() == upper)
        .cloned()
        .collect();
    let chosen = if key == upper || !stale.iter().any(|existing| existing == &upper) {
        key
    } else {
        upper
    };
    for existing in stale {
        target.remove(&existing);
    }
    target.insert(chosen, value);
}

fn remove_case_aware(
    target: &mut BTreeMap<String, String>,
    key: &str,
    is_windows: bool,
) -> Option<String> {
    if !is_windows {
        return target.remove(key);
    }
    let matches: Vec<String> = target
        .keys()
        .filter(|existing| existing.eq_ignore_ascii_case(key))
        .cloned()
        .collect();
    let mut removed = None;
    for matched in matches {
        removed = target.remove(&matched);
    }
    removed
}

/// Sanitized environment map ready to apply to a spawned child process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SanitizedChildEnv {
    is_windows: bool,
    vars: BTreeMap<String, String>,
}

impl SanitizedChildEnv {
    /// Builds a [`SanitizedChildEnv`] strictly from caller-supplied `(key, value)` entries.
    ///
    /// Strips `WATCH_REPORT_DEPENDENCIES`, `NODE_WATCH_*`, and Node debugger flags inside
    /// `NODE_OPTIONS`.
    pub fn from_entries<I, K, V>(entries: I, is_windows: bool) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        let mut env = Self {
            is_windows,
            vars: BTreeMap::new(),
        };
        for (key, value) in entries {
            env.set(key, value);
        }
        env
    }

    /// Captures and sanitizes the current host process environment.
    pub fn from_os_env() -> Self {
        Self::from_entries(std::env::vars(), cfg!(windows))
    }

    /// Inserts or updates a variable while enforcing child-env sanitization rules.
    pub fn set(&mut self, key: impl Into<String>, value: impl Into<String>) {
        let key = key.into();
        let value = value.into();
        if is_blocked_child_env_key(&key) {
            remove_case_aware(&mut self.vars, &key, self.is_windows);
            return;
        }
        if is_node_options_key(&key, self.is_windows) {
            match sanitize_node_options(&value) {
                Some(clean) => insert_case_aware(&mut self.vars, key, clean, self.is_windows),
                None => {
                    remove_case_aware(&mut self.vars, &key, self.is_windows);
                }
            }
            return;
        }
        insert_case_aware(&mut self.vars, key, value, self.is_windows);
    }

    /// Removes a variable using platform casing semantics.
    pub fn remove(&mut self, key: &str) -> Option<String> {
        remove_case_aware(&mut self.vars, key, self.is_windows)
    }

    /// Looks up a variable value, matching ASCII case-insensitively when configured for Windows.
    pub fn get(&self, key: &str) -> Option<&str> {
        if !self.is_windows {
            return self.vars.get(key).map(String::as_str);
        }
        if let Some(exact) = self.vars.get(key) {
            return Some(exact.as_str());
        }
        self.vars
            .iter()
            .find(|(existing, _)| existing.eq_ignore_ascii_case(key))
            .map(|(_, value)| value.as_str())
    }

    /// Returns a read-only view of the sanitized environment map.
    pub fn as_map(&self) -> &BTreeMap<String, String> {
        &self.vars
    }

    /// Consumes the wrapper and returns the underlying map.
    pub fn into_map(self) -> BTreeMap<String, String> {
        self.vars
    }

    /// Returns whether Windows case-insensitive normalization is active.
    pub fn is_windows(&self) -> bool {
        self.is_windows
    }

    /// Clears inherited variables on `command` and applies only this sanitized environment.
    pub fn apply_to_command(&self, command: &mut Command) {
        command.env_clear();
        command.envs(&self.vars);
    }

    /// Parses a JSON object mapping string keys to string values and returns a sanitized env.
    pub fn from_json_str(json: &str, is_windows: bool) -> Result<Self, serde_json::Error> {
        let raw: BTreeMap<String, String> = serde_json::from_str(json)?;
        Ok(Self::from_entries(raw, is_windows))
    }

    /// Serializes the sanitized environment map as compact JSON.
    pub fn to_json_string(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(&self.vars)
    }
}
