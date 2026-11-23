//! CLI probe binary lookup (including Windows `.cmd`/`.bat` shims) and version token parsing.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::drivers::antigravity::{
    AntigravityAuthConfig, AntigravityProbeOutcome, probe_antigravity,
};

/// Resolves `executable` against explicit `path_var` / `pathext_var` (respecting `BUG-12` explicit env).
#[must_use]
pub fn find_executable_on_path(
    executable: &str,
    path_var: Option<&str>,
    pathext_var: Option<&str>,
    is_windows: bool,
) -> Option<PathBuf> {
    let trimmed = executable.trim();
    if trimmed.is_empty() {
        return None;
    }
    let raw = Path::new(trimmed);
    if raw.components().count() > 1 || raw.is_absolute() {
        return raw.is_file().then(|| raw.to_path_buf());
    }
    let path_str = path_var?;
    let exts: Vec<&str> = if is_windows && raw.extension().is_none() {
        pathext_var
            .unwrap_or(".COM;.EXE;.BAT;.CMD")
            .split(';')
            .filter(|s| !s.is_empty())
            .collect()
    } else {
        vec![""]
    };
    for dir in std::env::split_paths(path_str) {
        for ext in &exts {
            let cand = if ext.is_empty() {
                dir.join(trimmed)
            } else {
                dir.join(format!("{trimmed}{ext}"))
            };
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}

/// Resolves executable and wraps Windows `.cmd`/`.bat` shims with `cmd.exe /D /C`.
#[must_use]
pub fn resolve_cli_probe_command(
    executable: &str,
    args: &[String],
    path_var: Option<&str>,
    pathext_var: Option<&str>,
    is_windows: bool,
) -> Option<(String, Vec<String>)> {
    let resolved = find_executable_on_path(executable, path_var, pathext_var, is_windows)?;
    let path_str = resolved.to_string_lossy().into_owned();
    if is_windows
        && resolved
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"))
    {
        let mut wrapped = vec!["/D".to_owned(), "/C".to_owned(), path_str];
        wrapped.extend(args.iter().cloned());
        return Some(("cmd.exe".to_owned(), wrapped));
    }
    Some((path_str, args.to_vec()))
}

/// Extracts the first `major.minor...` semantic version token from `--version` probe output.
#[must_use]
pub fn extract_cli_version(output: &str) -> Option<String> {
    output
        .split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | '(' | ')'))
        .map(|tok| tok.trim_start_matches(['v', 'V']))
        .find(|tok| {
            let mut parts = tok.split('.');
            let major = parts
                .next()
                .is_some_and(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
            let minor = parts
                .next()
                .is_some_and(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
            major && minor
        })
        .map(ToOwned::to_owned)
}

/// Classifies stdout/stderr + exit success from a `--version` CLI probe into status, version, and summary line.
#[must_use]
pub fn classify_cli_probe_output(
    stdout: &str,
    stderr: &str,
    success: bool,
) -> (super::ProviderProbeStatus, Option<String>, String) {
    let combined = format!("{stdout}\n{stderr}").trim().to_owned();
    let lower = combined.to_ascii_lowercase();
    let status = if success {
        super::ProviderProbeStatus::Ready
    } else if lower.contains("unauthorized")
        || lower.contains("not logged in")
        || lower.contains("login")
    {
        super::ProviderProbeStatus::NeedsAuth
    } else {
        super::ProviderProbeStatus::Error
    };
    let version = (status == super::ProviderProbeStatus::Ready)
        .then(|| extract_cli_version(&combined))
        .flatten();
    let summary = combined.lines().next().unwrap_or("probe exited").to_owned();
    (status, version, summary)
}

/// Evaluates local Antigravity installation/auth and formats `(status, status_code, message, details)`.
#[must_use]
pub fn summarize_antigravity_probe(
    instance_key: &str,
    data_root: &str,
    temp_root: &str,
) -> (super::ProviderProbeStatus, Option<i32>, String, Value) {
    let auth = AntigravityAuthConfig::default();
    match probe_antigravity(None, &auth, instance_key, data_root, temp_root) {
        AntigravityProbeOutcome::NotInstalled { reason } => (
            super::ProviderProbeStatus::NotInstalled,
            None,
            reason,
            json!({ "installed": false, "authenticated": false }),
        ),
        AntigravityProbeOutcome::NeedsAuth { reason, target } => (
            super::ProviderProbeStatus::NeedsAuth,
            None,
            reason,
            json!({ "installed": true, "authenticated": false, "executable": target.executable() }),
        ),
        AntigravityProbeOutcome::Ready {
            target,
            directories,
        } => (
            super::ProviderProbeStatus::Ready,
            Some(0),
            format!("Antigravity ready at {}", target.executable()),
            json!({
                "installed": true,
                "authenticated": true,
                "executable": target.executable(),
                "geminiHome": directories.gemini_home,
            }),
        ),
    }
}
