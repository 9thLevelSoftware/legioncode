//! Per-pane content-addressed Ghostty theme file storage (`BUG-26`).

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const MAX_SANITIZED_PANE_LEN: usize = 48;

/// Stored theme file entry tracked for one terminal pane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeFileEntry {
    /// Original terminal pane identifier.
    pub pane_id: String,
    /// Content-addressed filename `t3code-ghostty-{sanitized_pane_id}-{sha256_prefix12}.conf`.
    pub filename: String,
    /// Full rendered Ghostty config text.
    pub contents: String,
    /// 12-character lowercase hex SHA-256 prefix of `contents`.
    pub sha256_prefix12: String,
}

/// Action plan returned by [`TerminalThemeFileStore::upsert`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeFileWritePlan {
    /// Target filename `t3code-ghostty-{sanitized_pane_id}-{sha256_prefix12}.conf`.
    pub filename: String,
    /// Config payload to persist when `changed` is `true`.
    pub contents: String,
    /// `true` when the pane had no prior file or its content hash changed.
    pub changed: bool,
    /// Previous filename owned by the same `pane_id` to garbage-collect after writing, if any.
    pub obsolete_filename: Option<String>,
}

/// Per-pane content-addressed Ghostty theme config registry (`BUG-26`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalThemeFileStore {
    entries: BTreeMap<String, ThemeFileEntry>,
}

impl TerminalThemeFileStore {
    /// Creates an empty per-pane theme store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Computes the deterministic per-pane filename
    /// `t3code-ghostty-{sanitized_pane_id}-{sha256_prefix12}.conf` without mutating state.
    pub fn filename_for(pane_id: &str, contents: &str) -> String {
        let sanitized = sanitize_pane_id(pane_id);
        let prefix = sha256_prefix12(contents);
        format!("t3code-ghostty-{sanitized}-{prefix}.conf")
    }

    /// Registers or updates the theme configuration for `pane_id`.
    ///
    /// Two different `pane_id`s always receive distinct filenames, preventing multi-pane clobbering.
    pub fn upsert(&mut self, pane_id: &str, contents: impl Into<String>) -> ThemeFileWritePlan {
        let contents = contents.into();
        let prefix = sha256_prefix12(&contents);
        let filename = format!("t3code-ghostty-{}-{prefix}.conf", sanitize_pane_id(pane_id));

        if let Some(existing) = self.entries.get(pane_id) {
            if existing.filename == filename && existing.contents == contents {
                return ThemeFileWritePlan {
                    filename,
                    contents,
                    changed: false,
                    obsolete_filename: None,
                };
            }
            let prev = existing.filename.clone();
            self.entries.insert(
                pane_id.to_owned(),
                ThemeFileEntry {
                    pane_id: pane_id.to_owned(),
                    filename: filename.clone(),
                    contents: contents.clone(),
                    sha256_prefix12: prefix,
                },
            );
            return ThemeFileWritePlan {
                filename,
                contents,
                changed: true,
                obsolete_filename: Some(prev),
            };
        }

        self.entries.insert(
            pane_id.to_owned(),
            ThemeFileEntry {
                pane_id: pane_id.to_owned(),
                filename: filename.clone(),
                contents: contents.clone(),
                sha256_prefix12: prefix,
            },
        );
        ThemeFileWritePlan {
            filename,
            contents,
            changed: true,
            obsolete_filename: None,
        }
    }

    /// Returns the active theme entry for `pane_id`, if registered.
    pub fn get(&self, pane_id: &str) -> Option<&ThemeFileEntry> {
        self.entries.get(pane_id)
    }

    /// Removes `pane_id` from the store, returning its entry so callers can delete the file.
    pub fn remove(&mut self, pane_id: &str) -> Option<ThemeFileEntry> {
        self.entries.remove(pane_id)
    }

    /// Number of tracked panes.
    pub fn pane_count(&self) -> usize {
        self.entries.len()
    }
}

/// Sanitizes a terminal `pane_id` into a single safe filename component (`[A-Za-z0-9_-]`).
pub fn sanitize_pane_id(pane_id: &str) -> String {
    let mut out = String::with_capacity(pane_id.len().min(MAX_SANITIZED_PANE_LEN));
    let mut last_dash = false;
    for ch in pane_id.chars() {
        if out.len() >= MAX_SANITIZED_PANE_LEN {
            break;
        }
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch);
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "default".to_owned()
    } else {
        out
    }
}

/// Renders a Ghostty theme config string with `font-size` and optional appearance marker.
pub fn render_ghostty_theme_config(
    theme_config: &str,
    font_size: f64,
    appearance: Option<&str>,
) -> String {
    let mut rendered = theme_config.trim_end_matches('\n').to_owned();
    if !rendered.is_empty() {
        rendered.push('\n');
    }
    let _ = writeln!(rendered, "font-size = {font_size}");
    if let Some(scheme) = appearance
        && !scheme.trim().is_empty()
    {
        let _ = writeln!(rendered, "t3-appearance = {}", scheme.trim());
    }
    rendered
}

fn sha256_prefix12(contents: &str) -> String {
    let digest = Sha256::digest(contents.as_bytes());
    let encoded = hex::encode(digest);
    encoded.chars().take(12).collect()
}
