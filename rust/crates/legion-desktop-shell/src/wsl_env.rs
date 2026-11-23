//! WSL runtime probe, staging, and pruning honoring distro login-shell `LEGIONCODE_HOME` (`#39771`).

use legion_contracts::brand::{ACTIVE_BRAND, BrandConfig};
use serde::{Deserialize, Serialize};

/// Output of probing a WSL distro login shell for PATH and home directory (`#39771`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WslProbeOutcome {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_path: Option<String>,
    /// Absolute POSIX home directory exported by the distro's login shell, or `None` when unset/relative.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legioncode_home: Option<String>,
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
/// (`legioncode_home`), or `$HOME/<brand.home_dir_name>/wsl-runtime` when unset (`#39771`).
pub fn runtime_parent_assignment_for_brand(
    legioncode_home: Option<&str>,
    brand: &BrandConfig,
) -> String {
    match legioncode_home
        .map(str::trim)
        .filter(|s| s.starts_with('/'))
    {
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
pub fn runtime_parent_assignment(legioncode_home: Option<&str>) -> String {
    runtime_parent_assignment_for_brand(legioncode_home, &ACTIVE_BRAND)
}

/// Builds the POSIX `bash -lc` probe script emitting `resolvedPath:<PATH>` and `legioncodeHome:<HOME>` (`#39771`).
pub fn build_wsl_login_probe_command_for_brand(brand: &BrandConfig) -> String {
    let candidates = brand.env_var_candidates("HOME");
    let first = candidates.first().map_or("LEGIONCODE_HOME", String::as_str);
    let fallback = candidates.get(1).map(String::as_str);
    let home_expr = if let Some(fb) = fallback {
        format!("${{{first}:-${{{fb}:-}}}}")
    } else {
        format!("${{{first}:-}}")
    };
    let inner = format!(
        "printf 'resolvedPath:%s\\n' \"$PATH\"; printf 'legioncodeHome:%s\\n' \"{home_expr}\""
    );
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

/// Parses `legioncodeHome:` from WSL login-shell probe output, returning `None` if empty or relative (`#39771`).
pub fn parse_wsl_legioncode_home(stdout: &str) -> Option<String> {
    parse_absolute_probe_field(stdout, "legioncodeHome")
}

/// Parses raw login-shell stdout into a structured [`WslProbeOutcome`] (`#39771`).
pub fn evaluate_wsl_probe_stdout(stdout: &str, exit_code: i32) -> WslProbeOutcome {
    if exit_code != 0 {
        return WslProbeOutcome {
            ok: false,
            resolved_path: None,
            legioncode_home: None,
            reason: Some(
                "WSL login-shell PATH could not be resolved during backend preflight.".to_owned(),
            ),
        };
    }
    let Some(resolved_path) = parse_wsl_resolved_path(stdout) else {
        return WslProbeOutcome {
            ok: false,
            resolved_path: None,
            legioncode_home: None,
            reason: Some("WSL login-shell PATH could not be resolved".to_owned()),
        };
    };
    let legioncode_home = parse_wsl_legioncode_home(stdout);
    WslProbeOutcome {
        ok: true,
        resolved_path: Some(resolved_path),
        legioncode_home,
        reason: None,
    }
}

/// Renders the POSIX WSL runtime cache install script honoring login-shell `legioncode_home` (`#39771`).
pub fn build_wsl_runtime_install_script(
    linux_archive_path: Option<&str>,
    runtime_id: &str,
    sha256: &str,
    legioncode_home: Option<&str>,
) -> String {
    let safe_id = sanitize_wsl_runtime_id(runtime_id);
    let parent = runtime_parent_assignment(legioncode_home);
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

/// Renders the POSIX WSL runtime cache prune script honoring login-shell `legioncode_home` (`#39771`).
pub fn build_wsl_runtime_prune_script(runtime_id: &str, legioncode_home: Option<&str>) -> String {
    let safe_id = sanitize_wsl_runtime_id(runtime_id);
    let parent = runtime_parent_assignment(legioncode_home);
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

/// Returns the environment pair to pass into `wsl.exe --exec` when `legioncode_home` is resolved (`#39771`).
pub fn wsl_home_launch_env(legioncode_home: Option<&str>) -> Option<(String, String)> {
    let abs = legioncode_home
        .map(str::trim)
        .filter(|s| s.starts_with('/'))?;
    Some((
        format!("{}HOME", ACTIVE_BRAND.primary_env_prefix),
        abs.to_owned(),
    ))
}

/// Registry keys registered by real Store/MSI (`WSLService`) or inbox (`LxssManager`) WSL installs
/// — absent when only the Windows `System32\wsl.exe` install stub exists (`#40918`).
pub const WSL_SERVICE_KEYS: [&str; 2] = [
    r"HKLM\SYSTEM\CurrentControlSet\Services\WSLService",
    r"HKLM\SYSTEM\CurrentControlSet\Services\LxssManager",
];

/// Returns `true` when at least one WSL service registry key is registered (`#40918`).
pub fn is_wsl_service_registered_with<F>(mut query_key: F) -> bool
where
    F: FnMut(&str) -> bool,
{
    WSL_SERVICE_KEYS.iter().any(|key| query_key(key))
}

/// Probes the Windows Service Control registry via `reg.exe query` for installed WSL services
/// (`#40918`). Always returns `false` on non-Windows targets.
#[must_use]
pub fn is_wsl_service_registered() -> bool {
    if !cfg!(target_os = "windows") {
        return false;
    }
    is_wsl_service_registered_with(|key| {
        std::process::Command::new("reg.exe")
            .args(["query", key])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    })
}

/// Evaluates whether WSL is actually usable on this machine, requiring `win32`, `wsl.exe`, a
/// registered WSL service (`WSLService` or `LxssManager`), and no blocking managed policy (`#40918`).
pub fn probe_wsl_availability_with<FExe, FReg, FPol>(
    platform: &str,
    windir: &str,
    wsl_exe_exists: FExe,
    reg_query_ok: FReg,
    has_managed_policy: FPol,
) -> bool
where
    FExe: FnOnce(&std::path::Path) -> bool,
    FReg: FnMut(&str) -> bool,
    FPol: FnOnce() -> bool,
{
    if platform != "win32" {
        return false;
    }
    let wsl_exe = std::path::Path::new(windir)
        .join("System32")
        .join("wsl.exe");
    if !wsl_exe_exists(&wsl_exe) {
        return false;
    }
    if !is_wsl_service_registered_with(reg_query_ok) {
        return false;
    }
    !has_managed_policy()
}

/// Host filesystem + Windows registry check implementing `DesktopWslEnvironment.makeIsAvailable` (`#40918`).
#[must_use]
pub fn is_wsl_available_on_host(platform: &str, windir: &str, has_managed_policy: bool) -> bool {
    probe_wsl_availability_with(
        platform,
        windir,
        std::path::Path::exists,
        |key| {
            cfg!(target_os = "windows")
                && std::process::Command::new("reg.exe")
                    .args(["query", key])
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .is_ok_and(|status| status.success())
        },
        || has_managed_policy,
    )
}
