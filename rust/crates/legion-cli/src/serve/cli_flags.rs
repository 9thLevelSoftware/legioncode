//! CLI argument parser for root `legioncode`, `legioncode start`, and `legioncode serve`.

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
            Self::Root => "legioncode",
            Self::Start => "legioncode start",
            Self::Serve => "legioncode serve",
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
    pub(crate) dev_url: Option<String>,
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
                return flag_err(
                    kind,
                    &format!("Unrecognized flag: {arg} in command {}", kind.cmd_label()),
                );
            }
            if state.cwd_arg.is_some() {
                return flag_err(kind, &format!("Unexpected argument: {arg}"));
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
    dev_url: Option<String>,
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
    } else if let Some(val) = read_opt_flag(kind, rest, idx, "home-dir")? {
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
            _ => return bad_val(kind, "--mode", &val, "Expected: \"web\" | \"desktop\""),
        });
    } else if let Some(val) = read_opt_flag(kind, rest, idx, "dev-url")? {
        if !val.starts_with("http://") && !val.starts_with("https://") {
            return bad_val(
                kind,
                "--dev-url",
                &val,
                "Expected an http:// or https:// URL.",
            );
        }
        acc.dev_url = Some(val.trim().trim_end_matches('/').to_owned());
    } else if let Some(val) = read_opt_flag(kind, rest, idx, "bootstrap-fd")? {
        if val.trim().parse::<u32>().is_err() {
            return bad_val(
                kind,
                "--bootstrap-fd",
                &val,
                "Expected a non-negative integer.",
            );
        }
    } else {
        return Ok(false);
    }
    Ok(true)
}

fn flag_err<T>(kind: ServerEntryKind, msg: &str) -> Result<T, i32> {
    Err(fail_with_command_help(kind.help_path(), msg))
}

fn bad_val<T>(kind: ServerEntryKind, flag: &str, val: &str, exp: &str) -> Result<T, i32> {
    Err(fail_with_command_help(
        kind.help_path(),
        &format!("Invalid value for flag {flag}: \"{val}\". {exp}"),
    ))
}

fn read_opt_flag(
    kind: ServerEntryKind,
    rest: &[String],
    idx: &mut usize,
    name: &str,
) -> Result<Option<String>, i32> {
    match take_flag_value(rest, idx, name) {
        Ok(v) => Ok(v),
        Err(()) => flag_err(kind, &format!("Missing value for flag --{name}")),
    }
}

fn parse_port_value(kind: ServerEntryKind, flag: &str, raw: &str) -> Result<u16, i32> {
    match raw.trim().parse::<u16>() {
        Ok(port) => Ok(port),
        Err(_) => bad_val(kind, flag, raw, "Expected an integer between 0 and 65535."),
    }
}

fn build_parsed_config(kind: ServerEntryKind, acc: FlagAcc) -> Result<ParsedServerCli, i32> {
    if kind == ServerEntryKind::Root
        && let Some(ref raw_cwd) = acc.cwd_arg
    {
        let trimmed = raw_cwd.trim();
        if !is_explicit_path_syntax(trimmed) && !Path::new(trimmed).is_dir() {
            return Err(fail_cli_error(&format!(
                "Unknown command \"{trimmed}\". Use \"legioncode --help\" for commands or an explicit path such as \"legioncode ./my-project\" for a new directory."
            )));
        }
    }

    let (base_dir, base_dir_is_explicit) = resolve_base_dir(acc.base_dir.as_deref());
    let mode = acc
        .mode
        .or_else(
            || match resolve_env_nonempty("LEGIONCODE_MODE").as_deref() {
                Some("web") => Some(ServeMode::Web),
                Some("desktop") => Some(ServeMode::Desktop),
                _ => None,
            },
        )
        .unwrap_or(ServeMode::Web);
    let port = acc
        .port
        .or_else(|| {
            legion_contracts::brand::ACTIVE_BRAND
                .read_env("PORT")
                .and_then(|v| v.trim().parse::<u16>().ok())
        })
        .unwrap_or(DEFAULT_PORT);
    let host = acc
        .host
        .or_else(|| legion_contracts::brand::ACTIVE_BRAND.read_env("HOST"));
    let env_b = |v: Option<bool>, k| v.or_else(|| resolve_env_bool(k)).unwrap_or(false);
    let lan = env_b(acc.lan, "LEGIONCODE_LAN");
    let no_browser = match kind {
        ServerEntryKind::Serve => true,
        ServerEntryKind::Root | ServerEntryKind::Start => acc
            .no_browser
            .or_else(|| resolve_env_bool("LEGIONCODE_NO_BROWSER"))
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
        && env_b(
            acc.auto_bootstrap,
            "LEGIONCODE_AUTO_BOOTSTRAP_PROJECT_FROM_CWD",
        );
    let tailscale_serve = env_b(acc.tailscale_serve, "LEGIONCODE_TAILSCALE_SERVE");
    let tailscale_serve_port = acc
        .tailscale_serve_port
        .or_else(|| {
            resolve_env_nonempty("LEGIONCODE_TAILSCALE_SERVE_PORT")?
                .parse()
                .ok()
        })
        .unwrap_or(DEFAULT_TAILSCALE_SERVE_PORT);
    let dev_url = acc
        .dev_url
        .or_else(|| resolve_env_nonempty("VITE_DEV_SERVER_URL"))
        .map(|u| u.trim_end_matches('/').to_owned());

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
        dev_url,
    })
}

fn is_explicit_path_syntax(value: &str) -> bool {
    let b = value.as_bytes();
    matches!(value, "." | ".." | "~")
        || value.contains(['/', '\\'])
        || (cfg!(windows) && b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':')
}

fn resolve_env_nonempty(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
}

fn resolve_env_bool(name: &str) -> Option<bool> {
    match resolve_env_nonempty(name)?.to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{ServerEntryKind, parse_server_cli_args};

    #[test]
    fn parses_home_dir_dev_url_and_serve_auto_bootstrap() -> Result<(), Box<dyn std::error::Error>>
    {
        let temp = tempfile::tempdir()?;
        let home = temp.path().join("custom-home");
        let run = |k, s: String| {
            parse_server_cli_args(k, &s.split('|').map(str::to_owned).collect::<Vec<_>>())
        };
        let p = run(ServerEntryKind::Root, format!("--home-dir={}|--dev-url=http://127.0.0.1:5173/|--auto-bootstrap-project-from-cwd|--no-browser", home.display())).map_err(|c| format!("exit {c}"))?;
        assert!(
            p.config.base_dir == home
                && p.config.base_dir_is_explicit
                && p.auto_bootstrap
                && p.config.no_browser
        );
        assert_eq!(p.dev_url.as_deref(), Some("http://127.0.0.1:5173"));
        let s = run(
            ServerEntryKind::Serve,
            format!("--home-dir={}|--port|0", home.display()),
        )
        .map_err(|c| format!("exit {c}"))?;
        assert!(!s.auto_bootstrap && s.config.no_browser);
        Ok(())
    }
}
