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

fn win_parent_dir(path_str: &str) -> Option<&str> {
    let no_trailing = path_str.trim_end_matches(['\\', '/']);
    let idx = no_trailing.rfind(['\\', '/'])?;
    Some(&no_trailing[..idx])
}

/// Finds Git for Windows' `bash.exe` beside `git.exe` (never WSL's `System32\bash.exe`) (`#40418`).
#[must_use]
pub fn git_for_windows_bash(
    git_executable: &str,
    mut is_file: impl FnMut(&str) -> bool,
) -> Option<String> {
    let trimmed = git_executable.trim();
    let basename = trimmed.rsplit(['\\', '/']).next()?;
    if !basename.eq_ignore_ascii_case("git.exe") {
        return None;
    }
    let sep = if trimmed.contains('\\') { '\\' } else { '/' };
    let bin_or_cmd = win_parent_dir(trimmed)?;
    let launcher_root = win_parent_dir(bin_or_cmd)?;
    let grand_root = win_parent_dir(launcher_root);
    let mut roots = vec![launcher_root];
    if let Some(outer) = grand_root {
        roots.push(outer);
    }
    for root in roots {
        let candidate = format!("{root}{sep}bin{sep}bash.exe");
        if is_file(&candidate) {
            return Some(candidate);
        }
    }
    None
}

/// Shell-aware invocation built for ACP `terminal/create` requests (`#40418`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpTerminalInvocation {
    /// Executable program to spawn (Git Bash on Windows when `posix_shell` applies).
    pub program: String,
    /// Arguments passed directly without ambient shell wrapper when `use_default_shell` is `false`.
    pub args: Vec<String>,
    /// True when generic host shell wrapping should be enabled.
    pub use_default_shell: bool,
}

/// Builds an ACP terminal spawn plan running shell-source commands under Git Bash on Windows (`#40418`).
#[must_use]
pub fn build_acp_terminal_invocation(
    command: &str,
    args: &[String],
    shell_commands: bool,
    posix_shell: Option<&str>,
) -> AcpTerminalInvocation {
    let is_shell_source = shell_commands && args.is_empty();
    if is_shell_source && let Some(shell) = posix_shell {
        return AcpTerminalInvocation {
            program: shell.to_owned(),
            args: vec![
                "-c".to_owned(),
                format!("chcp.com 65001 >/dev/null 2>&1\n{command}"),
            ],
            use_default_shell: false,
        };
    }
    AcpTerminalInvocation {
        program: command.to_owned(),
        args: args.to_vec(),
        use_default_shell: is_shell_source,
    }
}

/// Merges `PATH` strings with deduplication while preserving order and platform separator (`#40693`).
#[must_use]
pub fn merge_path_entries(
    primary: Option<&str>,
    secondary: Option<&str>,
    is_windows: bool,
) -> String {
    let sep = if is_windows { ';' } else { ':' };
    let mut seen = std::collections::HashSet::new();
    let mut ordered = Vec::new();
    for raw in [primary, secondary].into_iter().flatten() {
        for segment in raw.split(sep) {
            let trimmed = segment.trim();
            if trimmed.is_empty() {
                continue;
            }
            let key = if is_windows {
                trimmed.to_ascii_lowercase()
            } else {
                trimmed.to_owned()
            };
            if seen.insert(key) {
                ordered.push(trimmed.to_owned());
            }
        }
    }
    ordered.join(&sep.to_string())
}

/// Reads case-variant `PATH`/`Path`/`path` from an explicit environment lookup (`#40693`).
#[must_use]
pub fn resolve_host_path_value(env: &[(String, String)]) -> Option<String> {
    for target in ["PATH", "Path", "path"] {
        if let Some((_, val)) = env.iter().find(|(k, _)| k == target) {
            return Some(val.clone());
        }
    }
    None
}

/// Compares previous and newly refreshed host `PATH` strings and returns whether instances
/// missing their CLI should rebuild on the updated `PATH` (`#40693`).
#[must_use]
pub fn did_host_path_change(previous_path: Option<&str>, current_path: Option<&str>) -> bool {
    previous_path != current_path
}

/// Returns candidate POSIX login shells to probe when refreshing `PATH` after launch (`#40693`).
#[must_use]
pub fn list_login_shell_candidates(configured_shell: Option<&str>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    let defaults = ["/bin/zsh", "/bin/bash", "/bin/sh"];
    for cand in configured_shell
        .into_iter()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .chain(defaults)
    {
        if seen.insert(cand.to_owned()) {
            out.push(cand.to_owned());
        }
    }
    out
}

/// Selects only provider instance IDs that are currently missing their CLI executable so
/// installed healthy instances preserve live sessions on `PATH` refresh (`#40693`).
#[must_use]
pub fn instances_needing_rebuild_after_path_change<'a>(
    instances: &[(&'a str, bool)],
) -> Vec<&'a str> {
    instances
        .iter()
        .filter_map(|&(id, installed)| (!installed).then_some(id))
        .collect()
}
