//! Guarded theme file reading, schema validation, and `themes/` directory loading.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use serde_json::Value;

use crate::theme::{
    EnvironmentThemeFile, MAX_THEME_FILE_BYTES, MAX_THEME_FILES, MAX_THEME_TOTAL_BYTES,
    PublishedThemeEntry, THEME_CSS_MAX_LENGTH, ThemeAppearance, ThemeError, ThemeVariants,
    is_reserved_theme_id, is_valid_theme_id,
};

/// Safely reads a UTF-8 theme file without following symlinks or reading past `max_bytes`.
///
/// Strips a leading UTF-8 BOM (`\u{FEFF}`) when present and returns both raw bytes and decoded UTF-8 text.
///
/// # Errors
///
/// Returns [`ThemeError::FileSymlink`], [`ThemeError::FileNotRegular`], [`ThemeError::FileTooLarge`], or [`ThemeError::FileRead`].
pub fn read_theme_file_guarded(
    path: &Path,
    max_bytes: usize,
) -> Result<(Vec<u8>, String), ThemeError> {
    let link_meta = fs::symlink_metadata(path).map_err(|err| ThemeError::FileRead {
        path: path.to_path_buf(),
        reason: err.to_string(),
    })?;
    if link_meta.file_type().is_symlink() {
        return Err(ThemeError::FileSymlink {
            path: path.to_path_buf(),
        });
    }
    if !link_meta.file_type().is_file() {
        return Err(ThemeError::FileNotRegular {
            path: path.to_path_buf(),
        });
    }
    let max_u64 = u64::try_from(max_bytes).unwrap_or(u64::MAX);
    if link_meta.len() > max_u64 {
        return Err(ThemeError::FileTooLarge {
            path: path.to_path_buf(),
            max_bytes,
        });
    }

    let file = File::open(path).map_err(|err| ThemeError::FileRead {
        path: path.to_path_buf(),
        reason: err.to_string(),
    })?;
    let opened_meta = file.metadata().map_err(|err| ThemeError::FileRead {
        path: path.to_path_buf(),
        reason: err.to_string(),
    })?;
    if !opened_meta.file_type().is_file() {
        return Err(ThemeError::FileNotRegular {
            path: path.to_path_buf(),
        });
    }
    if opened_meta.len() > max_u64 {
        return Err(ThemeError::FileTooLarge {
            path: path.to_path_buf(),
            max_bytes,
        });
    }

    let mut raw_bytes = Vec::new();
    file.take(max_u64.saturating_add(1))
        .read_to_end(&mut raw_bytes)
        .map_err(|err| ThemeError::FileRead {
            path: path.to_path_buf(),
            reason: err.to_string(),
        })?;
    if raw_bytes.len() > max_bytes {
        return Err(ThemeError::FileTooLarge {
            path: path.to_path_buf(),
            max_bytes,
        });
    }

    let text = String::from_utf8(raw_bytes.clone()).map_err(|err| ThemeError::FileRead {
        path: path.to_path_buf(),
        reason: err.to_string(),
    })?;
    let stripped = text.strip_prefix('\u{FEFF}').unwrap_or(&text).to_owned();
    Ok((raw_bytes, stripped))
}

/// Reads and strictly validates a custom theme JSON file, ensuring it defines colors.
///
/// # Errors
///
/// Returns [`ThemeError`] if the file cannot be read safely, fails schema validation, or has no colors.
pub fn read_and_validate_theme_file(
    path: &Path,
) -> Result<(Vec<u8>, EnvironmentThemeFile), ThemeError> {
    let (raw_bytes, text) = read_theme_file_guarded(path, MAX_THEME_FILE_BYTES)?;
    let theme = validate_theme_file_json(path, &text)?;
    if !theme.has_colors() {
        return Err(ThemeError::FileColorless {
            path: path.to_path_buf(),
        });
    }
    Ok((raw_bytes, theme))
}

/// Validates theme JSON text against `EnvironmentThemeFile` rules (does not enforce `has_colors`).
///
/// # Errors
///
/// Returns [`ThemeError::FileInvalid`] on JSON parse or schema constraint failure.
pub fn validate_theme_file_json(
    path: &Path,
    json_text: &str,
) -> Result<EnvironmentThemeFile, ThemeError> {
    let value: Value = serde_json::from_str(json_text).map_err(|err| ThemeError::FileInvalid {
        path: path.to_path_buf(),
        reason: err.to_string(),
    })?;
    let obj = value.as_object().ok_or_else(|| ThemeError::FileInvalid {
        path: path.to_path_buf(),
        reason: String::from("theme root must be a JSON object"),
    })?;

    let version = match obj.get("version") {
        None => None,
        Some(raw_ver) => {
            if raw_ver.as_u64() != Some(1) {
                return Err(ThemeError::FileInvalid {
                    path: path.to_path_buf(),
                    reason: String::from("version must equal 1 when present"),
                });
            }
            Some(1u8)
        }
    };

    let raw_name =
        obj.get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| ThemeError::FileInvalid {
                path: path.to_path_buf(),
                reason: String::from("name must be a non-empty string"),
            })?;
    let name = raw_name.trim().to_owned();
    if name.is_empty() || name.encode_utf16().count() > 48 {
        return Err(ThemeError::FileInvalid {
            path: path.to_path_buf(),
            reason: String::from("name must be 1..=48 characters after trimming"),
        });
    }

    let appearance = match obj.get("appearance").and_then(Value::as_str) {
        Some("light") => ThemeAppearance::Light,
        Some("dark") => ThemeAppearance::Dark,
        _ => {
            return Err(ThemeError::FileInvalid {
                path: path.to_path_buf(),
                reason: String::from("appearance must be \"light\" or \"dark\""),
            });
        }
    };

    let canvas = parse_optional_hex_color(obj.get("canvas"), "canvas", path)?;
    let accent = parse_optional_hex_color(obj.get("accent"), "accent", path)?;
    let colors = obj
        .get("colors")
        .map(|raw| parse_color_map(raw, "colors", path))
        .transpose()?;
    let variants = obj
        .get("variants")
        .map(|raw| parse_variants(raw, path))
        .transpose()?;
    let css = obj
        .get("css")
        .map(|raw| {
            let text = raw.as_str().ok_or_else(|| ThemeError::FileInvalid {
                path: path.to_path_buf(),
                reason: String::from("css must be a string"),
            })?;
            if text.encode_utf16().count() > THEME_CSS_MAX_LENGTH {
                return Err(ThemeError::FileInvalid {
                    path: path.to_path_buf(),
                    reason: format!("css must be <= {THEME_CSS_MAX_LENGTH} characters"),
                });
            }
            Ok(text.to_owned())
        })
        .transpose()?;

    Ok(EnvironmentThemeFile {
        version,
        name,
        appearance,
        canvas,
        accent,
        colors,
        variants,
        css,
    })
}

/// Scans `themes_dir` and returns valid published custom themes sorted by `id`.
#[must_use]
pub fn read_published_themes(themes_dir: &Path) -> Vec<PublishedThemeEntry> {
    let Ok(entries) = fs::read_dir(themes_dir) else {
        return Vec::new();
    };
    let mut candidates: Vec<(String, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(name_str) = file_name.to_str() else {
            continue;
        };
        let Some(id) = name_str.strip_suffix(".json") else {
            continue;
        };
        if !is_valid_theme_id(id) || is_reserved_theme_id(id) {
            continue;
        }
        candidates.push((id.to_owned(), entry.path()));
    }
    candidates.sort_by(|left, right| left.0.cmp(&right.0));

    let mut result = Vec::new();
    let mut total_bytes: usize = 0;
    for (id, path) in candidates {
        if result.len() >= MAX_THEME_FILES {
            break;
        }
        let Ok((raw_bytes, theme)) = read_and_validate_theme_file(&path) else {
            continue;
        };
        let next_total = total_bytes.saturating_add(raw_bytes.len());
        if next_total > MAX_THEME_TOTAL_BYTES {
            continue;
        }
        total_bytes = next_total;
        result.push(PublishedThemeEntry { id, theme });
    }
    result
}

fn hex_color_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let compiled = Regex::new(r"^#(?:[0-9a-fA-F]{3}|[0-9a-fA-F]{6})$");
        // INFALLIBLE: static compile-time constant hex color regex.
        compiled.expect("static hex color regex")
    })
}

fn color_key_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let compiled = Regex::new(r"^[a-zA-Z][a-zA-Z0-9]{0,63}$");
        // INFALLIBLE: static compile-time constant CSS token key regex.
        compiled.expect("static color key regex")
    })
}

fn parse_optional_hex_color(
    raw: Option<&Value>,
    field: &str,
    path: &Path,
) -> Result<Option<String>, ThemeError> {
    let Some(val) = raw else {
        return Ok(None);
    };
    let s = val.as_str().ok_or_else(|| ThemeError::FileInvalid {
        path: path.to_path_buf(),
        reason: format!("{field} must be a hex color string"),
    })?;
    if !hex_color_regex().is_match(s) {
        return Err(ThemeError::FileInvalid {
            path: path.to_path_buf(),
            reason: format!("{field} must match #RGB or #RRGGBB"),
        });
    }
    Ok(Some(s.to_owned()))
}

fn parse_color_map(
    raw: &Value,
    field: &str,
    path: &Path,
) -> Result<BTreeMap<String, String>, ThemeError> {
    let obj = raw.as_object().ok_or_else(|| ThemeError::FileInvalid {
        path: path.to_path_buf(),
        reason: format!("{field} must be a JSON object"),
    })?;
    let mut map = BTreeMap::new();
    for (key, value) in obj {
        if !color_key_regex().is_match(key) {
            return Err(ThemeError::FileInvalid {
                path: path.to_path_buf(),
                reason: format!("{field}.{key} is not a valid color token key"),
            });
        }
        let raw_val = value.as_str().ok_or_else(|| ThemeError::FileInvalid {
            path: path.to_path_buf(),
            reason: format!("{field}.{key} must be a string"),
        })?;
        let trimmed = raw_val.trim();
        if trimmed.is_empty() || trimmed.encode_utf16().count() > 64 {
            return Err(ThemeError::FileInvalid {
                path: path.to_path_buf(),
                reason: format!("{field}.{key} must be 1..=64 characters after trimming"),
            });
        }
        map.insert(key.clone(), trimmed.to_owned());
    }
    Ok(map)
}

fn parse_variants(raw: &Value, path: &Path) -> Result<ThemeVariants, ThemeError> {
    let obj = raw.as_object().ok_or_else(|| ThemeError::FileInvalid {
        path: path.to_path_buf(),
        reason: String::from("variants must be a JSON object"),
    })?;
    let light = obj
        .get("light")
        .map(|v| parse_color_map(v, "variants.light", path))
        .transpose()?;
    let dark = obj
        .get("dark")
        .map(|v| parse_color_map(v, "variants.dark", path))
        .transpose()?;
    Ok(ThemeVariants { light, dark })
}
