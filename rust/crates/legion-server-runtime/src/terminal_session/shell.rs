//! Default interactive OS shell discovery and child terminal environment construction.

use std::collections::{BTreeMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use legion_process_ffi::env::SanitizedChildEnv;

/// Resolves ordered default shell candidates for launching an interactive PTY on this OS.
///
/// On Windows, inspects `COMSPEC`, `pwsh.exe`, `powershell.exe`, and `cmd.exe`.
/// On Unix, inspects `SHELL` (from overrides or environment), `/bin/bash`, and `/bin/sh`.
pub fn resolve_default_shell_candidates(env_overrides: &BTreeMap<String, String>) -> Vec<OsString> {
    let host_env: BTreeMap<String, String> = std::env::vars().collect();
    resolve_shell_candidates_for_os(cfg!(windows), env_overrides, &host_env)
}

/// Resolves the highest-priority default shell executable on this OS.
pub fn resolve_default_shell(env_overrides: &BTreeMap<String, String>) -> OsString {
    resolve_default_shell_candidates(env_overrides)
        .into_iter()
        .next()
        .unwrap_or_else(|| {
            if cfg!(windows) {
                OsString::from("cmd.exe")
            } else {
                OsString::from("/bin/sh")
            }
        })
}

/// Pure OS-parameterized shell resolution helper for deterministic cross-platform tests.
pub(crate) fn resolve_shell_candidates_for_os(
    is_windows: bool,
    env_overrides: &BTreeMap<String, String>,
    host_env: &BTreeMap<String, String>,
) -> Vec<OsString> {
    let path_var = lookup_env(env_overrides, host_env, "PATH", is_windows);
    let pathext_var = lookup_env(env_overrides, host_env, "PATHEXT", is_windows);
    let mut raw = Vec::new();

    if is_windows {
        if let Some(comspec) = lookup_env(env_overrides, host_env, "COMSPEC", true) {
            push_if_non_empty(&mut raw, comspec);
        }
        raw.push("pwsh.exe");
        raw.push("powershell.exe");
        raw.push("cmd.exe");
    } else {
        if let Some(shell) = lookup_env(env_overrides, host_env, "SHELL", false) {
            push_if_non_empty(&mut raw, shell);
        }
        raw.push("/bin/bash");
        raw.push("/bin/sh");
    }

    let mut seen = HashSet::new();
    let mut resolved = Vec::new();
    let mut deferred = Vec::new();

    for candidate in raw {
        let trimmed = candidate.trim();
        if trimmed.is_empty() {
            continue;
        }
        let dedup_key = if is_windows {
            trimmed.to_ascii_lowercase()
        } else {
            trimmed.to_owned()
        };
        if !seen.insert(dedup_key) {
            continue;
        }
        if let Some(found) = find_executable(trimmed, path_var, pathext_var, is_windows) {
            let found_str = found.to_string_lossy().into_owned();
            let norm = if is_windows {
                found_str.to_ascii_lowercase()
            } else {
                found_str
            };
            seen.insert(norm);
            resolved.push(found.into_os_string());
        } else {
            deferred.push(OsString::from(trimmed));
        }
    }

    resolved.extend(deferred);
    resolved
}

fn push_if_non_empty<'a>(out: &mut Vec<&'a str>, val: &'a str) {
    let trimmed = val.trim();
    if !trimmed.is_empty() {
        out.push(trimmed);
    }
}

fn lookup_env<'a>(
    overrides: &'a BTreeMap<String, String>,
    host: &'a BTreeMap<String, String>,
    key: &str,
    case_insensitive: bool,
) -> Option<&'a str> {
    if !case_insensitive {
        return overrides
            .get(key)
            .or_else(|| host.get(key))
            .map(String::as_str);
    }
    overrides
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .or_else(|| host.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)))
        .map(|(_, v)| v.as_str())
}

fn find_executable(
    candidate: &str,
    path_var: Option<&str>,
    pathext_var: Option<&str>,
    is_windows: bool,
) -> Option<PathBuf> {
    let candidate_path = Path::new(candidate);
    if candidate_path.components().count() > 1 || candidate_path.is_absolute() {
        return candidate_path
            .is_file()
            .then(|| candidate_path.to_path_buf());
    }
    let path_str = path_var?;
    let extensions = windows_extensions(candidate, pathext_var, is_windows);
    for dir in std::env::split_paths(path_str) {
        for ext in &extensions {
            let joined = if ext.is_empty() {
                dir.join(candidate)
            } else {
                dir.join(format!("{candidate}{ext}"))
            };
            if joined.is_file() {
                return Some(joined);
            }
        }
    }
    None
}

fn windows_extensions<'a>(
    candidate: &str,
    pathext_var: Option<&'a str>,
    is_windows: bool,
) -> Vec<&'a str> {
    if !is_windows || Path::new(candidate).extension().is_some() {
        return vec![""];
    }
    let raw = pathext_var.unwrap_or(".COM;.EXE;.BAT;.CMD");
    let mut exts: Vec<&str> = raw.split(';').filter(|s| !s.is_empty()).collect();
    if exts.is_empty() {
        exts.push(".EXE");
    }
    exts
}

/// Builds a sanitized child environment for interactive terminal sessions (`BUG-09`).
pub(crate) fn build_terminal_child_env(
    env_overrides: &BTreeMap<String, String>,
) -> SanitizedChildEnv {
    let mut env = SanitizedChildEnv::from_os_env();
    if env.get("TERM").is_none() {
        env.set("TERM", "xterm-256color");
    }
    if env.get("COLORTERM").is_none() {
        env.set("COLORTERM", "truecolor");
    }
    if env.get("LEGIONCODE_TERMINAL").is_none() {
        env.set("LEGIONCODE_TERMINAL", "1");
    }
    for (k, v) in env_overrides {
        env.set(k.as_str(), v.as_str());
    }
    env
}
