//! Lenient JSONC parsing and optimistic compare-and-swap updates for `settings.json`.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use serde_json::{Map, Value};
use uuid::Uuid;

use crate::theme::{CONCURRENT_WRITE_ATTEMPTS, ThemeError};

/// Reads the configured `"defaultTheme"` string from `settings.json`, tolerating JSONC comments and trailing commas.
///
/// Returns `Ok(None)` when `settings.json` does not exist, is empty, or has no `"defaultTheme"` string.
///
/// # Errors
///
/// Returns [`ThemeError::SettingsUnreadable`] on I/O errors or [`ThemeError::SettingsMalformed`] on invalid JSONC.
pub fn read_default_theme(settings_path: &Path) -> Result<Option<String>, ThemeError> {
    let Some(raw) = read_settings_raw_optional(settings_path)? else {
        return Ok(None);
    };
    let map = parse_settings_jsonc_object(settings_path, &raw)?;
    Ok(map
        .get("defaultTheme")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned))
}

/// Sets `"defaultTheme"` and `"defaultThemeSetAt"` in `settings.json` using optimistic compare-and-swap.
///
/// # Errors
///
/// Returns [`ThemeError`] if `settings.json` is malformed/unreadable or fails CAS after [`CONCURRENT_WRITE_ATTEMPTS`].
pub fn set_default_theme(
    settings_path: &Path,
    theme_id: &str,
    now_ms: i64,
) -> Result<(), ThemeError> {
    set_default_theme_with_hook(settings_path, theme_id, now_ms, |_| {})
}

/// Like [`set_default_theme`], invoking `before_cas_check(attempt)` immediately before the CAS re-read.
///
/// Intended for testing concurrent modifications cleanly without race-prone sleeps.
///
/// # Errors
///
/// Returns [`ThemeError`] if reading, parsing, CAS validation, or writing `settings.json` fails.
pub fn set_default_theme_with_hook<F>(
    settings_path: &Path,
    theme_id: &str,
    now_ms: i64,
    mut before_cas_check: F,
) -> Result<(), ThemeError>
where
    F: FnMut(usize),
{
    let set_at = format_iso_utc_ms(now_ms);
    mutate_settings_cas(settings_path, &mut before_cas_check, |map| {
        map.insert(
            String::from("defaultTheme"),
            Value::String(theme_id.to_owned()),
        );
        map.insert(
            String::from("defaultThemeSetAt"),
            Value::String(set_at.clone()),
        );
    })
}

/// Removes `"defaultTheme"` and `"defaultThemeSetAt"` from `settings.json` using optimistic compare-and-swap.
///
/// # Errors
///
/// Returns [`ThemeError`] if `settings.json` is malformed/unreadable or fails CAS after [`CONCURRENT_WRITE_ATTEMPTS`].
pub fn clear_default_theme(settings_path: &Path) -> Result<(), ThemeError> {
    mutate_settings_cas(settings_path, &mut |_| {}, |map| {
        map.remove("defaultTheme");
        map.remove("defaultThemeSetAt");
    })
}

fn mutate_settings_cas<F, M>(
    settings_path: &Path,
    before_cas_check: &mut F,
    mut mutate: M,
) -> Result<(), ThemeError>
where
    F: FnMut(usize),
    M: FnMut(&mut Map<String, Value>),
{
    if let Some(parent) = settings_path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|err| ThemeError::SettingsUnreadable {
            path: settings_path.to_path_buf(),
            reason: err.to_string(),
        })?;
    }

    for attempt in 1..=CONCURRENT_WRITE_ATTEMPTS {
        let initial_raw = read_settings_raw_optional(settings_path)?;
        let mut map = match initial_raw.as_deref() {
            None => Map::new(),
            Some(raw) => parse_settings_jsonc_object(settings_path, raw)?,
        };
        mutate(&mut map);

        let serialized = serde_json::to_string_pretty(&Value::Object(map)).map_err(|err| {
            ThemeError::SettingsMalformed {
                path: settings_path.to_path_buf(),
                reason: err.to_string(),
            }
        })?;
        let output = format!("{serialized}\n");

        before_cas_check(attempt);

        let current_raw = read_settings_raw_optional(settings_path)?;
        if current_raw != initial_raw {
            continue;
        }

        write_atomic_settings(settings_path, output.as_bytes())?;
        return Ok(());
    }

    Err(ThemeError::SettingsBusy {
        path: settings_path.to_path_buf(),
        attempts: CONCURRENT_WRITE_ATTEMPTS,
    })
}

fn read_settings_raw_optional(settings_path: &Path) -> Result<Option<String>, ThemeError> {
    match fs::read_to_string(settings_path) {
        Ok(raw) => Ok(Some(raw)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(ThemeError::SettingsUnreadable {
            path: settings_path.to_path_buf(),
            reason: err.to_string(),
        }),
    }
}

fn parse_settings_jsonc_object(
    settings_path: &Path,
    raw: &str,
) -> Result<Map<String, Value>, ThemeError> {
    if raw.trim().is_empty() {
        return Ok(Map::new());
    }
    let clean = strip_jsonc(raw);
    let value: Value =
        serde_json::from_str(&clean).map_err(|err| ThemeError::SettingsMalformed {
            path: settings_path.to_path_buf(),
            reason: err.to_string(),
        })?;
    value
        .as_object()
        .cloned()
        .ok_or_else(|| ThemeError::SettingsMalformed {
            path: settings_path.to_path_buf(),
            reason: String::from("settings root must be a JSON object"),
        })
}

fn write_atomic_settings(settings_path: &Path, bytes: &[u8]) -> Result<(), ThemeError> {
    let parent = settings_path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = settings_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("settings.json");
    let tmp_path = parent.join(format!("{file_name}.tmp-{}", Uuid::new_v4()));

    #[cfg(unix)]
    let existing_mode = {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(settings_path)
            .ok()
            .map(|m| m.permissions().mode() & 0o777)
    };

    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(existing_mode.unwrap_or(0o600));
    }

    let mut file = opts
        .open(&tmp_path)
        .map_err(|err| ThemeError::SettingsUnreadable {
            path: settings_path.to_path_buf(),
            reason: err.to_string(),
        })?;

    if let Err(err) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        let _ = fs::remove_file(&tmp_path);
        return Err(ThemeError::SettingsUnreadable {
            path: settings_path.to_path_buf(),
            reason: err.to_string(),
        });
    }
    drop(file);

    if let Err(err) = fs::rename(&tmp_path, settings_path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(ThemeError::SettingsUnreadable {
            path: settings_path.to_path_buf(),
            reason: err.to_string(),
        });
    }
    Ok(())
}

/// Strips JSONC `//` line comments, `/* */` block comments, and trailing commas outside strings.
fn strip_jsonc(input: &str) -> String {
    let mut no_comments = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    let mut in_string = false;
    let mut escaped = false;

    while i < chars.len() {
        let c = chars[i];
        if in_string {
            no_comments.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            i += 1;
            continue;
        }

        if c == '"' {
            in_string = true;
            no_comments.push(c);
            i += 1;
            continue;
        }

        if c == '/' && i + 1 < chars.len() && chars[i + 1] == '/' {
            i += 2;
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }

        if c == '/' && i + 1 < chars.len() && chars[i + 1] == '*' {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            if i + 1 < chars.len() {
                i += 2;
            } else {
                i = chars.len();
            }
            continue;
        }

        no_comments.push(c);
        i += 1;
    }

    strip_trailing_commas(&no_comments)
}

fn strip_trailing_commas(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    let mut in_string = false;
    let mut escaped = false;

    while i < chars.len() {
        let c = chars[i];
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            i += 1;
            continue;
        }

        if c == '"' {
            in_string = true;
            out.push(c);
            i += 1;
            continue;
        }

        if c == ',' {
            let mut look = i + 1;
            while look < chars.len() && chars[look].is_whitespace() {
                look += 1;
            }
            if look < chars.len() && (chars[look] == '}' || chars[look] == ']') {
                i += 1;
                continue;
            }
        }

        out.push(c);
        i += 1;
    }
    out
}

fn format_iso_utc_ms(epoch_ms: i64) -> String {
    crate::trace_format::format_epoch_ms_iso(epoch_ms)
}
