//! CLI argument parser for root `t3`, `t3 start`, and `t3 serve`.

use std::env;
use std::fs;
use std::path::Path;

use super::config::{
    DEFAULT_PORT, ServeConfig, ServeMode, resolve_default_static_dir, resolve_static_dir_env,
};
use crate::cli_global::{fail_cli_error, fail_with_command_help};
use crate::fallback::{expand_home, resolve_base_dir, take_flag_value};

const DEFAULT_TAILSCALE_SERVE_PORT: u16 = 443;

/// Selects which top-level command invoked the server entrypoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ServerEntryKind {
    Root,
    Start,
    Serve,
}

impl ServerEntryKind {
    fn help_path(self) -> &'static [&'static str] {
        match self {
            Self::Root => &[],
            Self::Start => &["start"],
            Self::Serve => &["serve"],
        }
    }

    fn cmd_label(self) -> &'static str {
        match self {
            Self::Root => "t3",
            Self::Start => "t3 start",
            Self::Serve => "t3 serve",
        }
    }

    fn allow_auto_bootstrap(self) -> bool {
        !matches!(self, Self::Serve)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ParsedServerCli {
    pub(crate) config: ServeConfig,
    pub(crate) auto_bootstrap: bool,
    pub(crate) tailscale_serve: bool,
    pub(crate) tailscale_serve_port: u16,
}

pub(crate) fn parse_server_cli_args(
    kind: ServerEntryKind,
    rest: &[String],
) -> Result<ParsedServerCli, i32> {
    let mut state = FlagAcc::default();
    let mut idx = 0;

    while idx < rest.len() {
        let arg = &rest[idx];
        if !apply_server_flag(kind, rest, &mut idx, arg, &mut state)? {
            if arg.starts_with('-') {
                return Err(fail_with_command_help(
                    kind.help_path(),
                    &format!("Unrecognized flag: {arg} in command {}", kind.cmd_label()),
                ));
            }
            if state.cwd_arg.is_some() {
                return Err(fail_with_command_help(
                    kind.help_path(),
                    &format!("Unexpected argument: {arg}"),
                ));
            }
            state.cwd_arg = Some(arg.clone());
        }
        idx += 1;
    }

    build_parsed_config(kind, state)
}

#[derive(Default)]
struct FlagAcc {
    base_dir: Option<String>,
    port: Option<u16>,
    host: Option<String>,
    static_dir: Option<String>,
    mode: Option<ServeMode>,
    no_browser: Option<bool>,
    lan: Option<bool>,
    auto_bootstrap: Option<bool>,
    tailscale_serve: Option<bool>,
    tailscale_serve_port: Option<u16>,
    cwd_arg: Option<String>,
}

fn apply_server_flag(
    kind: ServerEntryKind,
    rest: &[String],
    idx: &mut usize,
    arg: &str,
    acc: &mut FlagAcc,
) -> Result<bool, i32> {
    match arg {
        "--no-browser" => acc.no_browser = Some(true),
        "--lan" => acc.lan = Some(true),
        "--tailscale-serve" => acc.tailscale_serve = Some(true),
        "--log-websocket-events" | "--log-ws-events" => {}
        "--auto-bootstrap-project-from-cwd" if kind.allow_auto_bootstrap() => {
            acc.auto_bootstrap = Some(true);
        }
        _ => return apply_valued_server_flag(kind, rest, idx, acc),
    }
    Ok(true)
}

fn apply_valued_server_flag(
    kind: ServerEntryKind,
    rest: &[String],
    idx: &mut usize,
    acc: &mut FlagAcc,
) -> Result<bool, i32> {
    if let Some(val) = read_opt_flag(kind, rest, idx, "base-dir")? {
        acc.base_dir = Some(val);
    } else if let Some(val) = read_opt_flag(kind, rest, idx, "host")? {
        acc.host = Some(val);
    } else if let Some(val) = read_opt_flag(kind, rest, idx, "static-dir")? {
        acc.static_dir = Some(val);
    } else if let Some(val) = read_opt_flag(kind, rest, idx, "port")? {
        acc.port = Some(parse_port_value(kind, "--port", &val)?);
    } else if let Some(val) = read_opt_flag(kind, rest, idx, "tailscale-serve-port")? {
        acc.tailscale_serve_port = Some(parse_port_value(kind, "--tailscale-serve-port", &val)?);
    } else if let Some(val) = read_opt_flag(kind, rest, idx, "mode")? {
        acc.mode = Some(match val.as_str() {
            "web" => ServeMode::Web,
            "desktop" => ServeMode::Desktop,
            _ => {
                return Err(fail_with_command_help(
                    kind.help_path(),
                    &format!(
                        "Invalid value for flag --mode: \"{val}\". Expected: \"web\" | \"desktop\""
                    ),
                ));
            }
        });
    } else if let Some(val) = read_opt_flag(kind, rest, idx, "dev-url")? {
        if !val.starts_with("http://") && !val.starts_with("https://") {
            return Err(fail_with_command_help(
                kind.help_path(),
                &format!(
                    "Invalid value for flag --dev-url: \"{val}\". Expected an http:// or https:// URL."
                ),
            ));
        }
    } else if let Some(val) = read_opt_flag(kind, rest, idx, "bootstrap-fd")? {
        if val.trim().parse::<u32>().is_err() {
            return Err(fail_with_command_help(
                kind.help_path(),
                &format!(
                    "Invalid value for flag --bootstrap-fd: \"{val}\". Expected a non-negative integer."
                ),
            ));
        }
    } else {
        return Ok(false);
    }
    Ok(true)
}

fn read_opt_flag(
    kind: ServerEntryKind,
    rest: &[String],
    idx: &mut usize,
    name: &str,
) -> Result<Option<String>, i32> {
    take_flag_value(rest, idx, name).map_err(|()| {
        fail_with_command_help(
            kind.help_path(),
            &format!("Missing value for flag --{name}"),
        )
    })
}

fn parse_port_value(kind: ServerEntryKind, flag: &str, raw: &str) -> Result<u16, i32> {
    raw.trim().parse::<u16>().map_err(|_| {
        fail_with_command_help(
            kind.help_path(),
            &format!(
                "Invalid value for flag {flag}: \"{raw}\". Expected an integer between 0 and 65535."
            ),
        )
    })
}

fn build_parsed_config(kind: ServerEntryKind, acc: FlagAcc) -> Result<ParsedServerCli, i32> {
    if kind == ServerEntryKind::Root
        && let Some(ref raw_cwd) = acc.cwd_arg
    {
        let trimmed = raw_cwd.trim();
        if !is_explicit_path_syntax(trimmed) && !Path::new(trimmed).is_dir() {
            return Err(fail_cli_error(&format!(
                "Unknown command \"{trimmed}\". Use \"t3 --help\" for commands or an explicit path such as \"t3 ./my-project\" for a new directory."
            )));
        }
    }

    let (base_dir, base_dir_is_explicit) = resolve_base_dir(acc.base_dir.as_deref());
    let mode = acc.mode.or_else(resolve_env_mode).unwrap_or(ServeMode::Web);
    let port = acc
        .port
        .or_else(|| resolve_env_u16("T3CODE_PORT"))
        .unwrap_or(DEFAULT_PORT);
    let host = acc.host.or_else(|| resolve_env_nonempty("T3CODE_HOST"));
    let lan = acc
        .lan
        .or_else(|| resolve_env_bool("T3CODE_LAN"))
        .unwrap_or(false);
    let no_browser = match kind {
        ServerEntryKind::Serve => true,
        ServerEntryKind::Root | ServerEntryKind::Start => acc
            .no_browser
            .or_else(|| resolve_env_bool("T3CODE_NO_BROWSER"))
            .unwrap_or(mode == ServeMode::Desktop),
    };
    let cwd = acc.cwd_arg.map_or_else(
        || env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
        |raw| expand_home(raw.trim()),
    );
    let static_dir = acc
        .static_dir
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(expand_home)
        .or_else(resolve_static_dir_env)
        .or_else(|| resolve_default_static_dir(&cwd));
    if !cwd.exists()
        && let Err(err) = fs::create_dir_all(&cwd)
    {
        return Err(fail_cli_error(&format!(
            "Failed to create working directory `{}`: {err}",
            cwd.display()
        )));
    }
    let auto_bootstrap = kind.allow_auto_bootstrap()
        && acc
            .auto_bootstrap
            .or_else(|| resolve_env_bool("T3CODE_AUTO_BOOTSTRAP_PROJECT_FROM_CWD"))
            .unwrap_or(false);
    let tailscale_serve = acc
        .tailscale_serve
        .or_else(|| resolve_env_bool("T3CODE_TAILSCALE_SERVE"))
        .unwrap_or(false);
    let tailscale_serve_port = acc
        .tailscale_serve_port
        .or_else(|| resolve_env_u16("T3CODE_TAILSCALE_SERVE_PORT"))
        .unwrap_or(DEFAULT_TAILSCALE_SERVE_PORT);

    Ok(ParsedServerCli {
        config: ServeConfig {
            base_dir,
            base_dir_is_explicit,
            port,
            host,
            mode,
            no_browser,
            lan,
            static_dir,
            cwd,
        },
        auto_bootstrap,
        tailscale_serve,
        tailscale_serve_port,
    })
}

fn is_explicit_path_syntax(value: &str) -> bool {
    let has_drive = value.len() >= 2
        && value.as_bytes()[0].is_ascii_alphabetic()
        && value.as_bytes()[1] == b':';
    value == "."
        || value == ".."
        || value == "~"
        || value.contains('/')
        || value.contains('\\')
        || (cfg!(windows) && has_drive)
}

fn resolve_env_nonempty(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|val| val.trim().to_owned())
        .filter(|val| !val.is_empty())
}

fn resolve_env_u16(name: &str) -> Option<u16> {
    resolve_env_nonempty(name)?.parse::<u16>().ok()
}

fn resolve_env_bool(name: &str) -> Option<bool> {
    match resolve_env_nonempty(name)?.to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn resolve_env_mode() -> Option<ServeMode> {
    match resolve_env_nonempty("T3CODE_MODE")?.as_str() {
        "web" => Some(ServeMode::Web),
        "desktop" => Some(ServeMode::Desktop),
        _ => None,
    }
}
