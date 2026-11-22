//! Explicit sanitized environment capability struct blocking ambient env leaks.
//!
//! Fixes BUG-12 across `apps/server/src/policy/managedProviders.ts`,
//! `ProviderInstanceRegistryHydration.ts`,
//! `packages/provider-core/src/server/instanceEnvironment.ts`, and
//! `apps/server/src/terminal/Manager.ts`.
//!
//! [`SanitizedEnv`] can only be constructed from caller-supplied entries and
//! never reads `std::env::vars()` directly.

use std::collections::{BTreeMap, HashSet};

use legion_contracts::brand::ACTIVE_BRAND;

/// Terminal environment variables stripped when launching child shells/providers.
pub const TERMINAL_ENV_BLOCKLIST: &[&str] = &[
    "PORT",
    "ELECTRON_RENDERER_PORT",
    "ELECTRON_RUN_AS_NODE",
    "TMUX",
    "TMUX_PANE",
    "STY",
    "WINDOW",
    "ZELLIJ",
    "ZELLIJ_SESSION_NAME",
    "ZELLIJ_PANE_ID",
];

/// `AppImage` wrapper variables scrubbed before spawning external shells or CLIs.
pub const APPIMAGE_ENV_VARS: &[&str] = &["APPIMAGE", "APPDIR", "ARGV0", "OWD"];

const APPDIR_PATH_LIST_KEYS: &[&str] = &[
    "PATH",
    "LD_LIBRARY_PATH",
    "XDG_DATA_DIRS",
    "GSETTINGS_SCHEMA_DIR",
];

/// Returns `true` when `key` belongs to an internal/ambient server variable that must not leak.
pub fn is_blocked_ambient_env_key(key: &str) -> bool {
    let upper = key.trim().to_ascii_uppercase();
    if upper.is_empty() {
        return true;
    }
    let brand_match = upper.starts_with(ACTIVE_BRAND.primary_env_prefix);
    if brand_match || upper.starts_with("VITE_") {
        return true;
    }
    TERMINAL_ENV_BLOCKLIST.contains(&upper.as_str()) || APPIMAGE_ENV_VARS.contains(&upper.as_str())
}

fn path_separator(is_windows: bool) -> char {
    if is_windows { ';' } else { ':' }
}

fn strip_appdir_segments(value: &str, appdir: &str, is_windows: bool) -> String {
    let clean_appdir = appdir.trim_end_matches(['/', '\\']);
    if clean_appdir.is_empty() {
        return value.to_owned();
    }
    let sep = path_separator(is_windows);
    let kept: Vec<&str> = value
        .split(sep)
        .filter(|entry| {
            let trimmed = entry.trim();
            if trimmed.is_empty() {
                return false;
            }
            if is_windows {
                !trimmed
                    .to_ascii_lowercase()
                    .starts_with(&clean_appdir.to_ascii_lowercase())
            } else {
                !trimmed.starts_with(clean_appdir)
            }
        })
        .collect();
    kept.join(&sep.to_string())
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
    let existing_keys: Vec<String> = target
        .keys()
        .filter(|existing| existing.to_ascii_uppercase() == upper)
        .cloned()
        .collect();

    if existing_keys.is_empty() {
        target.insert(key, value);
        return;
    }

    let chosen_key = if key == upper {
        key
    } else if existing_keys.iter().any(|existing| existing == &upper) {
        upper
    } else {
        key
    };
    for stale in existing_keys {
        target.remove(&stale);
    }
    target.insert(chosen_key, value);
}

fn dedupe_windows_case(entries: Vec<(String, String)>) -> BTreeMap<String, String> {
    let mut grouped: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for (key, value) in entries {
        grouped
            .entry(key.to_ascii_uppercase())
            .or_default()
            .push((key, value));
    }

    let mut out = BTreeMap::new();
    for (upper, variants) in grouped {
        if let Some((_, val)) = variants.iter().rev().find(|(k, _)| k == &upper) {
            out.insert(upper, val.clone());
        } else if let Some((k, val)) = variants.into_iter().next_back() {
            out.insert(k, val);
        }
    }
    out
}

/// Sanitized environment map safe to pass to provider subprocesses and PTYs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SanitizedEnv {
    is_windows: bool,
    vars: BTreeMap<String, String>,
}

impl SanitizedEnv {
    /// Builds a [`SanitizedEnv`] strictly from caller-supplied `(key, value)` pairs.
    ///
    /// Strips `LEGIONCODE_*`, `VITE_*`, [`TERMINAL_ENV_BLOCKLIST`], `AppImage` keys,
    /// and any `APPDIR`-prefixed entries inside `PATH`/`LD_LIBRARY_PATH`/`XDG_DATA_DIRS`/`GSETTINGS_SCHEMA_DIR`.
    pub fn from_entries<I, K, V>(entries: I, is_windows: bool) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        let raw_list: Vec<(String, String)> = entries
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect();

        let appdir = raw_list
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("APPDIR"))
            .map(|(_, v)| v.trim().to_owned())
            .filter(|v| !v.is_empty());

        let filtered: Vec<(String, String)> = raw_list
            .into_iter()
            .filter(|(key, _)| !is_blocked_ambient_env_key(key))
            .map(|(key, value)| {
                let adjusted = if let Some(ref dir) = appdir
                    && APPDIR_PATH_LIST_KEYS
                        .iter()
                        .any(|path_key| key.eq_ignore_ascii_case(path_key))
                {
                    strip_appdir_segments(&value, dir, is_windows)
                } else {
                    value
                };
                (key, adjusted)
            })
            .filter(|(key, value)| {
                !(value.is_empty()
                    && appdir.is_some()
                    && APPDIR_PATH_LIST_KEYS
                        .iter()
                        .any(|path_key| key.eq_ignore_ascii_case(path_key)))
            })
            .collect();

        let vars = if is_windows {
            dedupe_windows_case(filtered)
        } else {
            filtered.into_iter().collect()
        };

        Self { is_windows, vars }
    }

    /// Derives a child environment for a PTY shell, ignoring blocked ambient keys in `overrides`.
    #[must_use]
    pub fn for_terminal<I, K, V>(&self, overrides: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        let mut next = self.clone();
        for (key, value) in overrides {
            let k = key.into();
            if !is_blocked_ambient_env_key(&k) {
                insert_case_aware(&mut next.vars, k, value.into(), next.is_windows);
            }
        }
        next
    }

    /// Derives a child environment for a provider instance, applying instance and managed overrides.
    ///
    /// Both override layers filter `LEGIONCODE_*`, `VITE_*`, and blocked runtime variables;
    /// `managed_overrides` cannot be bypassed by `instance_overrides`.
    #[must_use]
    pub fn for_provider_instance<I1, I2, K1, V1, K2, V2>(
        &self,
        instance_overrides: I1,
        managed_overrides: I2,
    ) -> Self
    where
        I1: IntoIterator<Item = (K1, V1)>,
        K1: Into<String>,
        V1: Into<String>,
        I2: IntoIterator<Item = (K2, V2)>,
        K2: Into<String>,
        V2: Into<String>,
    {
        let mut next = self.clone();
        let mut locked_upper: HashSet<String> = HashSet::new();
        let managed_collected: Vec<(String, String)> = managed_overrides
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .filter(|(k, _)| !is_blocked_ambient_env_key(k))
            .collect();
        for (k, _) in &managed_collected {
            locked_upper.insert(if next.is_windows {
                k.to_ascii_uppercase()
            } else {
                k.clone()
            });
        }

        for (key, value) in instance_overrides {
            let k = key.into();
            if is_blocked_ambient_env_key(&k) {
                continue;
            }
            let lock_id = if next.is_windows {
                k.to_ascii_uppercase()
            } else {
                k.clone()
            };
            if !locked_upper.contains(&lock_id) {
                insert_case_aware(&mut next.vars, k, value.into(), next.is_windows);
            }
        }

        for (k, v) in managed_collected {
            insert_case_aware(&mut next.vars, k, v, next.is_windows);
        }
        next
    }

    /// Looks up a variable value, matching ASCII case-insensitively on Windows.
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
            .map(|(_, val)| val.as_str())
    }

    /// Returns an immutable view of the sanitized key-value map.
    pub fn as_map(&self) -> &BTreeMap<String, String> {
        &self.vars
    }

    /// Consumes the capability wrapper and yields the underlying environment map.
    pub fn into_map(self) -> BTreeMap<String, String> {
        self.vars
    }

    /// Returns whether Windows case-insensitive variable normalization is active.
    pub fn is_windows(&self) -> bool {
        self.is_windows
    }
}
