//! Fast `legioncode status --json` implementation and `service.env` / read-only boot service inspection.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{BufWriter, ErrorKind, Write};
use std::path::{Path, PathBuf};

use legion_edge::boot_service_units::{decode_service_unit_bytes, extract_service_unit_base_dir};
use serde::{Deserialize, Serialize};

use crate::cli_global::fail_with_command_help;
use crate::cmd_connect::read_cloud_status_flag_pair;
use crate::fallback::{delegate_to_node, resolve_base_dir, take_flag_value, unit_forced_original};

/// Unit identifier for `RUST_ANYTHING_UNIT_STATUS` and fallback logs.
pub const UNIT_STATUS: &str = "STATUS";

#[derive(Debug, Clone, PartialEq, Eq)]
struct StatusFlags {
    base_dir: Option<String>,
    json: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ServiceStatusJson {
    pub(crate) supported: bool,
    pub(crate) installed: bool,
    pub(crate) installed_version: Option<String>,
    pub(crate) other_base_dir: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LanStatusJson {
    pub(crate) enabled: bool,
    pub(crate) inherited_default: bool,
    pub(crate) host_override: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StatusOutputJson {
    pub(crate) service: ServiceStatusJson,
    pub(crate) connect: Option<serde_json::Value>,
    pub(crate) lan: LanStatusJson,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServiceStatePartial {
    active_version: Option<String>,
}

/// Runs `legioncode status`, supporting both human output and `--json` natively (including cloud state).
pub fn run(args: &[String]) -> i32 {
    if unit_forced_original(UNIT_STATUS) {
        return delegate_to_node(UNIT_STATUS, args);
    }

    let flags = match parse_status_flags(args.get(1..).unwrap_or(&[])) {
        Ok(parsed) => parsed,
        Err(code) => return code,
    };

    let (base_dir, _) = resolve_base_dir(flags.base_dir.as_deref());
    let env_entries = match read_service_env_file(&base_dir.join("service.env")) {
        Ok(entries) => entries,
        Err(err) => {
            eprintln!("Failed to read service.env: {err}");
            return 1;
        }
    };

    let lan_opt = env_entries
        .get("LEGIONCODE_LAN")
        .and_then(|raw| parse_service_env_bool(raw));
    let host_override = env_entries
        .get("LEGIONCODE_HOST")
        .map(|val| val.trim().to_owned())
        .filter(|val| !val.is_empty());

    let service = detect_boot_service_status(&base_dir);
    let serves_this_home = service.installed && service.other_base_dir.is_none();
    let lan_enabled = lan_opt.unwrap_or(serves_this_home);
    let inherited_default = lan_opt.is_none() && serves_this_home && host_override.is_none();
    let connect = read_cloud_status_flag_pair(flags.base_dir.as_deref())
        .map(|(enabled, revoked)| serde_json::json!({ "enabled": enabled, "revoked": revoked }));

    let output = StatusOutputJson {
        service,
        connect,
        lan: LanStatusJson {
            enabled: lan_enabled,
            inherited_default,
            host_override,
        },
    };

    if !flags.json {
        return crate::cmd_status_human::print_human_status(flags.base_dir.as_deref(), &output);
    }

    let mut out = BufWriter::with_capacity(8192, std::io::stdout().lock());
    if let Err(err) = serde_json::to_writer_pretty(&mut out, &output)
        .map_err(std::io::Error::other)
        .and_then(|()| out.write_all(b"\n"))
        .and_then(|()| out.flush())
    {
        eprintln!("Failed to write status JSON: {err}");
        return 1;
    }
    0
}

fn parse_status_flags(rest: &[String]) -> Result<StatusFlags, i32> {
    let path = &["status"];
    let mut base_dir = None;
    let mut json = false;
    let mut idx = 0;

    while idx < rest.len() {
        let arg = &rest[idx];
        if arg == "--json" {
            json = true;
        } else {
            match take_flag_value(rest, &mut idx, "base-dir") {
                Ok(Some(val)) => base_dir = Some(val),
                Ok(None) if arg.starts_with('-') => {
                    return Err(fail_with_command_help(
                        path,
                        &format!("Unrecognized flag: {arg} in command legioncode status"),
                    ));
                }
                Ok(None) => {
                    return Err(fail_with_command_help(
                        path,
                        &format!("Unexpected argument: {arg}"),
                    ));
                }
                Err(()) => {
                    return Err(fail_with_command_help(
                        path,
                        "Missing value for flag --base-dir",
                    ));
                }
            }
        }
        idx += 1;
    }

    Ok(StatusFlags { base_dir, json })
}

fn read_service_env_file(path: &Path) -> std::io::Result<HashMap<String, String>> {
    match fs::read_to_string(path) {
        Ok(contents) => Ok(parse_service_env(&contents)),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(HashMap::new()),
        Err(err) => Err(err),
    }
}

/// Parses a `service.env` file (`KEY=VALUE` lines, ignoring `#` comments and stripping matching quotes).
///
/// ```
/// let parsed = legion_cli::cmd_status::parse_service_env("# note\nLEGIONCODE_LAN=\"yes\"\nLEGIONCODE_HOST=127.0.0.1\n");
/// assert_eq!(parsed.get("LEGIONCODE_LAN").map(String::as_str), Some("yes"));
/// assert_eq!(legion_cli::cmd_status::parse_service_env_bool("yes"), Some(true));
/// ```
pub fn parse_service_env(contents: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for raw_line in contents.lines() {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let body = trimmed
            .strip_prefix("export ")
            .map_or(trimmed, str::trim_start);
        let Some((raw_key, raw_val)) = body.split_once('=') else {
            continue;
        };
        let key = raw_key.trim();
        if key.is_empty() {
            continue;
        }
        let value = parse_env_value(raw_val.trim());
        map.insert(key.to_owned(), value);
    }
    map
}

fn parse_env_value(raw: &str) -> String {
    if let Some(first) = raw.chars().next()
        && (first == '"' || first == '\'' || first == '`')
        && let Some(end_offset) = raw[1..].find(first)
    {
        let after = raw[1 + end_offset + 1..].trim_start();
        if after.is_empty() || after.starts_with('#') {
            return raw[1..=end_offset].to_owned();
        }
    }
    for (idx, ch) in raw.char_indices() {
        if ch == '#' && idx > 0 && raw[..idx].ends_with(char::is_whitespace) {
            return raw[..idx].trim_end().to_owned();
        }
    }
    raw.to_owned()
}

/// Parses boolean flags in `service.env` (`true`/`1`/`yes`/`on` vs `false`/`0`/`no`/`off`).
pub fn parse_service_env_bool(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}

pub(crate) fn detect_boot_service_status(base_dir: &Path) -> ServiceStatusJson {
    let Some(unit_path) = boot_service_unit_path() else {
        return ServiceStatusJson {
            supported: false,
            installed: false,
            installed_version: None,
            other_base_dir: None,
        };
    };

    let Ok(bytes) = fs::read(&unit_path) else {
        return ServiceStatusJson {
            supported: true,
            installed: false,
            installed_version: None,
            other_base_dir: None,
        };
    };

    let unit_text = decode_service_unit_bytes(&bytes);
    let installed_base_dir = extract_service_unit_base_dir(&unit_text);
    let other_base_dir = installed_base_dir
        .as_deref()
        .filter(|installed| !paths_match(Path::new(installed), base_dir))
        .map(str::to_owned);

    let state_base = installed_base_dir.as_deref().map_or(base_dir, Path::new);
    let installed_version = read_installed_version(state_base);

    ServiceStatusJson {
        supported: true,
        installed: true,
        installed_version,
        other_base_dir,
    }
}

pub(crate) fn boot_service_unit_path() -> Option<PathBuf> {
    if let Some(override_path) =
        crate::invocation::non_empty_env("LEGIONCODE_BOOT_SERVICE_UNIT_PATH")
    {
        return Some(PathBuf::from(override_path));
    }
    #[cfg(target_os = "windows")]
    {
        let local_app_data = env::var("LOCALAPPDATA")
            .ok()
            .filter(|s| !s.trim().is_empty())?;
        let user_home = env::var("USERPROFILE")
            .or_else(|_| env::var("HOME"))
            .ok()
            .filter(|s| !s.trim().is_empty())?;
        let _ = user_home;
        Some(
            PathBuf::from(local_app_data)
                .join("legioncode")
                .join("legioncode-task.xml"),
        )
    }
    #[cfg(target_os = "macos")]
    {
        let home = env::var("HOME").ok().filter(|s| !s.trim().is_empty())?;
        Some(
            PathBuf::from(home)
                .join("Library")
                .join("LaunchAgents")
                .join("dev.legioncode.service.plist"),
        )
    }
    #[cfg(target_os = "linux")]
    {
        let home = env::var("HOME").ok().filter(|s| !s.trim().is_empty())?;
        let xdg = env::var("XDG_CONFIG_HOME").ok().unwrap_or_default();
        let config_dir = if Path::new(&xdg).is_absolute() {
            PathBuf::from(xdg)
        } else {
            PathBuf::from(home).join(".config")
        };
        Some(
            config_dir
                .join("systemd")
                .join("user")
                .join("legioncode.service"),
        )
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

pub(crate) fn paths_match(a: &Path, b: &Path) -> bool {
    let ca = fs::canonicalize(a).unwrap_or_else(|_| a.to_path_buf());
    let cb = fs::canonicalize(b).unwrap_or_else(|_| b.to_path_buf());
    #[cfg(windows)]
    {
        ca.to_string_lossy()
            .eq_ignore_ascii_case(&cb.to_string_lossy())
    }
    #[cfg(not(windows))]
    {
        ca == cb
    }
}

fn read_installed_version(base_dir: &Path) -> Option<String> {
    let state_path = base_dir.join("runtime").join("service-state.json");
    let raw = fs::read_to_string(state_path).ok()?;
    let parsed: ServiceStatePartial = serde_json::from_str(&raw).ok()?;
    parsed.active_version.filter(|v| !v.trim().is_empty())
}
