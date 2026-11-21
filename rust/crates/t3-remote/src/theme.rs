//! Environment theme validation, atomic publishing, and CLI formatting.
//!
//! Mirrors `packages/shared/src/themePalettes.ts`, `apps/server/src/environmentTheme.ts`,
//! and `apps/server/src/cli/theme.ts`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use crate::theme_publish::{PublishedThemeHandle, publish_theme_file};
pub use crate::theme_settings::{
    clear_default_theme, read_default_theme, set_default_theme, set_default_theme_with_hook,
};
pub use crate::theme_validate::{
    read_and_validate_theme_file, read_published_themes, read_theme_file_guarded,
    validate_theme_file_json,
};

/// Built-in web and desktop environment theme identifiers.
pub const BUILT_IN_THEME_IDS: [&str; 5] = ["t3-chat", "grove", "ocean", "ember", "iris"];

/// Default theme identifier on mobile clients.
pub const MOBILE_DEFAULT_THEME_ID: &str = "t3-code";

/// Theme identifiers shipped in mobile clients.
pub const MOBILE_THEME_IDS: [&str; 6] = ["t3-code", "t3-chat", "grove", "ocean", "ember", "iris"];

/// Theme identifiers reserved by the client runtime and unavailable for custom theme files.
pub const RESERVED_THEME_IDS: [&str; 13] = [
    "system",
    "light",
    "dark",
    "t3-chat",
    "grove",
    "ocean",
    "ember",
    "iris",
    "t3-chat-dark",
    "t3-grove",
    "t3-ocean",
    "t3-ember",
    "t3-iris",
];

/// Theme identifiers rejected when publishing a custom theme into `themes/`.
pub const UNPUBLISHABLE_THEME_IDS: [&str; 14] = [
    "system",
    "light",
    "dark",
    "t3-chat",
    "grove",
    "ocean",
    "ember",
    "iris",
    "t3-chat-dark",
    "t3-grove",
    "t3-ocean",
    "t3-ember",
    "t3-iris",
    "t3-code",
];

/// Maximum CSS payload length allowed in an environment theme file (in UTF-16 code units).
pub const THEME_CSS_MAX_LENGTH: usize = 100_000;

/// Maximum byte size allowed for a single published theme JSON file (`616_384` bytes).
pub const MAX_THEME_FILE_BYTES: usize = 6 * THEME_CSS_MAX_LENGTH + 16 * 1024;

/// Maximum number of published custom themes loaded from `themes/`.
pub const MAX_THEME_FILES: usize = 32;

/// Maximum aggregate bytes allowed across all published custom themes in `themes/`.
pub const MAX_THEME_TOTAL_BYTES: usize = 2 * 1024 * 1024;

/// Maximum optimistic compare-and-swap retry attempts when updating `settings.json`.
pub const CONCURRENT_WRITE_ATTEMPTS: usize = 5;

/// Errors raised while validating, publishing, or configuring environment themes.
#[derive(Debug, Error)]
pub enum ThemeError {
    /// Theme identifier failed syntax validation.
    #[error("invalid theme id `{id}`")]
    InvalidId {
        /// Rejected theme identifier.
        id: String,
    },
    /// Theme identifier belongs to [`RESERVED_THEME_IDS`] or [`UNPUBLISHABLE_THEME_IDS`].
    #[error("theme id `{id}` is reserved and cannot be published")]
    UnpublishableId {
        /// Rejected reserved theme identifier.
        id: String,
    },
    /// Theme source path or entry is a symbolic link, which is refused for safety.
    #[error("refusing to read symlink theme file `{path}`")]
    FileSymlink {
        /// Symlink path that was rejected.
        path: PathBuf,
    },
    /// Theme source path is not a regular file (for example a directory or FIFO).
    #[error("theme path `{path}` is not a regular file")]
    FileNotRegular {
        /// Rejected non-regular file path.
        path: PathBuf,
    },
    /// Theme file exceeds the allowed byte limit.
    #[error("theme file `{path}` exceeds maximum size of {max_bytes} bytes")]
    FileTooLarge {
        /// Oversized file path.
        path: PathBuf,
        /// Byte limit enforced by the reader.
        max_bytes: usize,
    },
    /// Theme file is unreadable on disk or invalid UTF-8.
    #[error("failed to read theme file `{path}`: {reason}")]
    FileRead {
        /// File path that failed to read.
        path: PathBuf,
        /// Human-readable error detail.
        reason: String,
    },
    /// Theme file failed JSON syntax or schema validation.
    #[error("invalid theme file `{path}`: {reason}")]
    FileInvalid {
        /// File path that failed validation.
        path: PathBuf,
        /// Validation error reason.
        reason: String,
    },
    /// Theme file matches the structural schema but defines no colors or canvas+accent pair.
    #[error("theme file `{path}` must define colors or both canvas and accent")]
    FileColorless {
        /// Colorless theme file path.
        path: PathBuf,
    },
    /// Publish target path already exists as a directory.
    #[error("theme destination `{path}` is a directory")]
    DestinationDirectory {
        /// Colliding destination directory path.
        path: PathBuf,
    },
    /// Atomic publish I/O error.
    #[error("failed to publish theme `{path}`: {source}")]
    PublishIo {
        /// Destination or staging path involved in the failure.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// `settings.json` could not be read or written.
    #[error("settings file `{path}` could not be accessed: {reason}")]
    SettingsUnreadable {
        /// Path to `settings.json`.
        path: PathBuf,
        /// I/O error reason.
        reason: String,
    },
    /// `settings.json` contains malformed JSONC or a non-object root value.
    #[error("settings file `{path}` is malformed: {reason}")]
    SettingsMalformed {
        /// Path to `settings.json`.
        path: PathBuf,
        /// Parse/validation reason.
        reason: String,
    },
    /// Optimistic compare-and-swap update to `settings.json` exhausted [`CONCURRENT_WRITE_ATTEMPTS`].
    #[error("settings file `{path}` changed concurrently after {attempts} attempts")]
    SettingsBusy {
        /// Path to `settings.json`.
        path: PathBuf,
        /// Number of failed CAS attempts.
        attempts: usize,
    },
}

/// Base light/dark appearance declared by a theme file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeAppearance {
    /// Light theme surface.
    Light,
    /// Dark theme surface.
    Dark,
}

/// Optional light/dark token override maps in a theme file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThemeVariants {
    /// Light-mode CSS custom property overrides.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub light: Option<BTreeMap<String, String>>,
    /// Dark-mode CSS custom property overrides.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dark: Option<BTreeMap<String, String>>,
}

/// Validated custom environment theme definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvironmentThemeFile {
    /// Optional schema version (`1`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<u8>,
    /// Display name (trimmed non-empty, `<= 48` UTF-16 code units).
    pub name: String,
    /// Base light or dark appearance.
    pub appearance: ThemeAppearance,
    /// Optional hex background color (`#RGB` or `#RRGGBB`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canvas: Option<String>,
    /// Optional hex accent color (`#RGB` or `#RRGGBB`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accent: Option<String>,
    /// Optional token map keyed by `[a-zA-Z][a-zA-Z0-9]{0,63}`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub colors: Option<BTreeMap<String, String>>,
    /// Optional per-appearance override token maps.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variants: Option<ThemeVariants>,
    /// Optional custom CSS rules (`<= 100_000` UTF-16 code units).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub css: Option<String>,
}

impl EnvironmentThemeFile {
    /// Returns `true` when the theme defines either both `canvas`+`accent` or a non-empty `colors` map.
    #[must_use]
    pub fn has_colors(&self) -> bool {
        (self.canvas.is_some() && self.accent.is_some())
            || self.colors.as_ref().is_some_and(|c| !c.is_empty())
    }
}

/// Validated custom theme loaded from a `themes/<id>.json` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedThemeEntry {
    /// Theme identifier derived from the `<id>.json` filename.
    pub id: String,
    /// Parsed and validated theme definition.
    pub theme: EnvironmentThemeFile,
}

/// Returns `true` when `id` is a valid custom theme identifier (`[a-z0-9][a-z0-9-]{0,47}`, excluding `system`/`light`/`dark`).
///
/// ```
/// use t3_remote::theme::is_valid_theme_id;
///
/// assert!(is_valid_theme_id("nord-night"));
/// assert!(!is_valid_theme_id("system"));
/// assert!(!is_valid_theme_id("My_Theme"));
/// ```
#[must_use]
pub fn is_valid_theme_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    if bytes.is_empty() || bytes.len() > 48 {
        return false;
    }
    if matches!(id, "system" | "light" | "dark") {
        return false;
    }
    if !(bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit()) {
        return false;
    }
    bytes[1..]
        .iter()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

/// Returns `true` if `id` is listed in [`RESERVED_THEME_IDS`].
#[must_use]
pub fn is_reserved_theme_id(id: &str) -> bool {
    RESERVED_THEME_IDS.contains(&id)
}

/// Returns `true` if `id` is listed in [`UNPUBLISHABLE_THEME_IDS`].
#[must_use]
pub fn is_unpublishable_theme_id(id: &str) -> bool {
    UNPUBLISHABLE_THEME_IDS.contains(&id)
}

/// Formats confirmation text when publishing a theme file and setting it as active (`cli/theme.ts`).
#[must_use]
pub fn format_theme_published_and_set(file_path: &Path, theme_id: &str) -> String {
    format!(
        "Published {} as \"{theme_id}\" and set it as the environment theme.\n",
        file_path.display()
    )
}

/// Formats confirmation text when setting the environment theme id (`cli/theme.ts`).
#[must_use]
pub fn format_theme_set(theme_id: &str, settings_path: &Path) -> String {
    format!(
        "Environment theme set to \"{theme_id}\" in {}.\n",
        settings_path.display()
    )
}

/// Formats confirmation text when clearing the environment theme (`cli/theme.ts`).
#[must_use]
pub fn format_theme_cleared(settings_path: &Path) -> String {
    format!(
        "Environment theme cleared in {}.\n",
        settings_path.display()
    )
}

/// Formats the `t3 theme show` status output (`cli/theme.ts:501-546`) with trailing newline.
#[must_use]
pub fn format_theme_show(
    default_theme: Option<&str>,
    published_ids: &[String],
    themes_dir: &Path,
) -> String {
    let line1 = match default_theme {
        None => String::from("Environment theme: not set."),
        Some(id) => format!("Environment theme: \"{id}\"."),
    };
    let line2 = if published_ids.is_empty() {
        format!(
            "Published themes: none (publish into {}).",
            themes_dir.display()
        )
    } else {
        let mut sorted = published_ids.to_vec();
        sorted.sort();
        format!("Published themes: {}.", sorted.join(", "))
    };
    format!("{line1}\n{line2}\n")
}
