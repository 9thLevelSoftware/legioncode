//! WSL runtime probe, staging, and pruning honoring distro login-shell `T3CODE_HOME` / `LEGION_HOME` (`#39771`).

use serde::{Deserialize, Serialize};
use t3_contracts::brand::{ACTIVE_BRAND, BrandConfig};

/// Output of probing a WSL distro login shell for PATH and home directory (`#39771`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WslProbeOutcome {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_path: Option<String>,
    /// Absolute POSIX home directory exported by the distro's login shell, or `None` when unset/relative.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub t3_home: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Validates and sanitizes a runtime ID used in POSIX cache directory names.
pub fn sanitize_wsl_runtime_id(runtime_id: &str) -> String {
    let cleaned: String = runtime_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "default".to_owned()
    } else {
        cleaned
    }
}

/// Quotes a string safely for POSIX single-quoted `sh` / `bash` interpolation.
pub fn shell_quote_posix(value: &str) -> String {
    let escaped = value.replace('\'', "'\"'\"'");
    format!("'{escaped}'")
}

/// Computes the `runtime_parent=...` shell assignment favoring an explicit login-shell home
/// (`t3_home`), or `$HOME/<brand.home_dir_name>/wsl-runtime` when unset (`#39771`).
pub fn runtime_parent_assignment_for_brand(t3_home: Option<&str>, brand: &BrandConfig) -> String {
    match t3_home.map(str::trim).filter(|s| s.starts_with('/')) {
        Some(abs) => format!(
            "runtime_parent={}",
            shell_quote_posix(&format!("{abs}/wsl-runtime"))
        ),
        None => format!(
            "runtime_parent=\"$HOME/{}/wsl-runtime\"",
            brand.home_dir_name
        ),
    }
}

/// Computes `runtime_parent=...` using [`ACTIVE_BRAND`].
pub fn runtime_parent_assignment(t3_home: Option<&str>) -> String {
    runtime_parent_assignment_for_brand(t3_home, &ACTIVE_BRAND)
}

/// Builds the POSIX `bash -lc` probe script emitting `resolvedPath:<PATH>` and `t3Home:<HOME>` (`#39771`).
pub fn build_wsl_login_probe_command_for_brand(brand: &BrandConfig) -> String {
    let candidates = brand.env_var_candidates("HOME");
    let first = candidates.first().map_or("T3CODE_HOME", String::as_str);
    let fallback = candidates.get(1).map(String::as_str);
    let home_expr = if let Some(fb) = fallback {
        format!("${{{first}:-${{{fb}:-}}}}")
    } else {
        format!("${{{first}:-}}")
    };
    let inner =
        format!("printf 'resolvedPath:%s\\n' \"$PATH\"; printf 't3Home:%s\\n' \"{home_expr}\"");
    format!(
        "bash -lc {} 2>/dev/null || {}",
        shell_quote_posix(&inner),
        inner
    )
}

/// Builds the WSL login-shell probe command using [`ACTIVE_BRAND`].
pub fn build_wsl_login_probe_command() -> String {
    build_wsl_login_probe_command_for_brand(&ACTIVE_BRAND)
}

/// Extracts an absolute POSIX field `<prefix>:<value>` from WSL probe stdout (`#39771`).
///
/// Relative paths are ignored because staging scripts and `wsl.exe --exec` launch from different cwds.
pub fn parse_absolute_probe_field(stdout: &str, prefix: &str) -> Option<String> {
    let needle = format!("{prefix}:");
    let mut found = None;
    for raw_line in stdout.lines() {
        let line = raw_line.trim();
        if let Some(rest) = line.strip_prefix(&needle) {
            let candidate = rest.trim();
            if candidate.starts_with('/') {
                found = Some(candidate.to_owned());
            } else if !candidate.is_empty() {
                found = None;
            }
        }
    }
    found
}

/// Parses `resolvedPath:` from WSL login-shell probe output (`#39771`).
pub fn parse_wsl_resolved_path(stdout: &str) -> Option<String> {
    for raw_line in stdout.lines() {
        let line = raw_line.trim();
        if let Some(rest) = line.strip_prefix("resolvedPath:") {
            let trimmed = rest.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_owned());
            }
        }
    }
    None
}

/// Parses `t3Home:` from WSL login-shell probe output, returning `None` if empty or relative (`#39771`).
pub fn parse_wsl_t3_home(stdout: &str) -> Option<String> {
    parse_absolute_probe_field(stdout, "t3Home")
}

/// Parses raw login-shell stdout into a structured [`WslProbeOutcome`] (`#39771`).
pub fn evaluate_wsl_probe_stdout(stdout: &str, exit_code: i32) -> WslProbeOutcome {
    if exit_code != 0 {
        return WslProbeOutcome {
            ok: false,
            resolved_path: None,
            t3_home: None,
            reason: Some(
                "WSL login-shell PATH could not be resolved during backend preflight.".to_owned(),
            ),
        };
    }
    let Some(resolved_path) = parse_wsl_resolved_path(stdout) else {
        return WslProbeOutcome {
            ok: false,
            resolved_path: None,
            t3_home: None,
            reason: Some("WSL login-shell PATH could not be resolved".to_owned()),
        };
    };
    let t3_home = parse_wsl_t3_home(stdout);
    WslProbeOutcome {
        ok: true,
        resolved_path: Some(resolved_path),
        t3_home,
        reason: None,
    }
}

/// Renders the POSIX WSL runtime cache install script honoring login-shell `t3_home` (`#39771`).
pub fn build_wsl_runtime_install_script(
    linux_archive_path: Option<&str>,
    runtime_id: &str,
    sha256: &str,
    t3_home: Option<&str>,
) -> String {
    let safe_id = sanitize_wsl_runtime_id(runtime_id);
    let parent = runtime_parent_assignment(t3_home);
    let archive_check = linux_archive_path.map_or_else(
        || "# probe existing cached runtime only".to_owned(),
        |path| {
            format!(
                "mkdir -p \"$runtime_dir\" && tar -xzf {} -C \"$runtime_dir\" # sha256:{sha256}",
                shell_quote_posix(path)
            )
        },
    );
    [
        "set -eu".to_owned(),
        parent,
        format!("runtime_dir=\"$runtime_parent/{safe_id}\""),
        archive_check,
        "printf 'staged:%s\\n' \"$runtime_dir\"".to_owned(),
    ]
    .join("\n")
}

/// Renders the POSIX WSL runtime cache prune script honoring login-shell `t3_home` (`#39771`).
pub fn build_wsl_runtime_prune_script(runtime_id: &str, t3_home: Option<&str>) -> String {
    let safe_id = sanitize_wsl_runtime_id(runtime_id);
    let parent = runtime_parent_assignment(t3_home);
    [
        "set -eu".to_owned(),
        parent,
        "if [ -d \"$runtime_parent\" ]; then".to_owned(),
        format!(
            "  for entry in \"$runtime_parent\"/*; do [ -e \"$entry\" ] || continue; [ \"$(basename \"$entry\")\" = {} ] || rm -rf \"$entry\"; done",
            shell_quote_posix(&safe_id)
        ),
        "fi".to_owned(),
    ]
    .join("\n")
}

/// Returns the environment pair to pass into `wsl.exe --exec` when `t3_home` is resolved (`#39771`).
pub fn wsl_home_launch_env(t3_home: Option<&str>) -> Option<(String, String)> {
    let abs = t3_home.map(str::trim).filter(|s| s.starts_with('/'))?;
    Some((
        format!("{}HOME", ACTIVE_BRAND.primary_env_prefix),
        abs.to_owned(),
    ))
}
