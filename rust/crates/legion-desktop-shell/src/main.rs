//! Binary entry point for the `legioncode-desktop` safe Rust Tauri v2 desktop host shell.

#![forbid(unsafe_code)]

use std::env;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use legion_desktop_shell::bundle::{
    DesktopBundleConfig, emit_packaging_bundle, render_tauri_config_json,
};
use legion_desktop_shell::desktop_shell_version;
use legion_desktop_shell::ipc::DesktopIpcRouter;
use legion_desktop_shell::self_check::run_self_check;
use legion_desktop_shell::window_host::{
    BaseDirOrigin, DesktopLaunchConfig, DesktopSidecarPolicy, launch_desktop_window,
};
use legion_persistence::paths::resolve_base_dir;

#[derive(Debug, Clone, PartialEq, Eq)]
enum CliAction {
    Help,
    Version,
    PrintTauriConfig,
    EmitBundle(String),
    IpcEval(String),
    SelfCheck,
    SmokeWebview,
    Launch {
        url: Option<String>,
        dev_url: Option<String>,
        static_dir: Option<PathBuf>,
        base_dir: Option<PathBuf>,
        enable_control_server: bool,
    },
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    let code = run(&args[1..]);
    ExitCode::from(code)
}

fn run(args: &[String]) -> u8 {
    let action = match parse_args(args) {
        Ok(action) => action,
        Err(msg) => {
            let _ = writeln!(io::stderr(), "{msg}");
            return 2;
        }
    };

    match action {
        CliAction::Help => print_stdout(&usage_text()),
        CliAction::Version => {
            print_stdout(&format!("legioncode-desktop v{}", desktop_shell_version()))
        }
        CliAction::PrintTauriConfig => {
            match render_tauri_config_json(&DesktopBundleConfig::default()) {
                Ok(json) => print_stdout_raw(&json),
                Err(err) => fail(&format!("failed to render tauri.conf.json: {err}")),
            }
        }
        CliAction::EmitBundle(dir) => match emit_packaging_bundle(Path::new(&dir)) {
            Ok(manifest) => match serde_json::to_string_pretty(&manifest) {
                Ok(json) => print_stdout(&json),
                Err(err) => fail(&format!("failed to serialize bundle manifest: {err}")),
            },
            Err(err) => fail(&format!("failed to emit packaging bundle: {err}")),
        },
        CliAction::IpcEval(json_input) => {
            let mut router = DesktopIpcRouter::default();
            match router.dispatch_json(&json_input) {
                Ok(resp) => match serde_json::to_string_pretty(&resp) {
                    Ok(json) => print_stdout(&json),
                    Err(err) => fail(&format!("failed to serialize IPC response: {err}")),
                },
                Err(err) => {
                    let _ = writeln!(io::stderr(), "invalid --ipc-eval JSON envelope: {err}");
                    2
                }
            }
        }
        CliAction::SelfCheck => match run_self_check() {
            Ok(report) => match serde_json::to_string_pretty(&report) {
                Ok(json) => print_stdout(&json),
                Err(err) => fail(&format!("failed to serialize self-check report: {err}")),
            },
            Err(err) => fail(&format!("self-check failed: {err}")),
        },
        CliAction::SmokeWebview => run_webview_host(&DesktopLaunchConfig {
            headless_smoke: true,
            ..DesktopLaunchConfig::default()
        }),
        CliAction::Launch {
            url,
            dev_url,
            static_dir,
            base_dir,
            enable_control_server,
        } => {
            let (resolved_base_dir, base_dir_is_explicit) = resolve_base_dir(base_dir.as_deref());
            let mut config = DesktopLaunchConfig {
                base_dir: resolved_base_dir,
                base_dir_origin: BaseDirOrigin::from_is_explicit(base_dir_is_explicit),
                dev_url,
                enable_control_server,
                sidecar_policy: if url.is_none() {
                    DesktopSidecarPolicy::ManagedLocal
                } else {
                    DesktopSidecarPolicy::Disabled
                },
                ..DesktopLaunchConfig::default()
            };
            if let Some(target_url) = url {
                config.initial_url = target_url;
            }
            if let Some(dir) = static_dir {
                config.static_dir = Some(dir);
            }
            run_webview_host(&config)
        }
    }
}

fn run_webview_host(config: &DesktopLaunchConfig) -> u8 {
    let smoke_mode = config.headless_smoke;
    match launch_desktop_window(config) {
        Ok(report) => match serde_json::to_string_pretty(&report) {
            Ok(json) => {
                let write_code = print_stdout(&json);
                if write_code != 0 {
                    return write_code;
                }
                if smoke_mode && !report.passed {
                    return 1;
                }
                0
            }
            Err(err) => fail(&format!("failed to serialize desktop run report: {err}")),
        },
        Err(err) => fail(&format!("desktop window launch failed: {err}")),
    }
}

fn parse_args(args: &[String]) -> Result<CliAction, String> {
    if args.is_empty() {
        return parse_launch_args(&[]);
    }
    match args[0].as_str() {
        "--help" | "-h" => {
            ensure_no_extra_args(args, 1)?;
            Ok(CliAction::Help)
        }
        "--version" | "-V" | "-v" => {
            ensure_no_extra_args(args, 1)?;
            Ok(CliAction::Version)
        }
        "--print-tauri-config" => {
            ensure_no_extra_args(args, 1)?;
            Ok(CliAction::PrintTauriConfig)
        }
        "--self-check" => {
            ensure_no_extra_args(args, 1)?;
            Ok(CliAction::SelfCheck)
        }
        "--smoke-webview" | "--headless-smoke" => {
            ensure_no_extra_args(args, 1)?;
            Ok(CliAction::SmokeWebview)
        }
        "--launch" => parse_launch_args(&args[1..]),
        "--emit-bundle" => {
            let Some(dir) = args.get(1) else {
                return Err("missing directory argument after --emit-bundle".to_owned());
            };
            ensure_no_extra_args(args, 2)?;
            Ok(CliAction::EmitBundle(dir.clone()))
        }
        "--ipc-eval" => {
            let Some(json) = args.get(1) else {
                return Err("missing JSON argument after --ipc-eval".to_owned());
            };
            ensure_no_extra_args(args, 2)?;
            Ok(CliAction::IpcEval(json.clone()))
        }
        arg if arg.starts_with("--emit-bundle=") => {
            ensure_no_extra_args(args, 1)?;
            let dir = arg.trim_start_matches("--emit-bundle=").to_owned();
            if dir.is_empty() {
                return Err("--emit-bundle=<dir> must not be empty".to_owned());
            }
            Ok(CliAction::EmitBundle(dir))
        }
        arg if arg.starts_with("--ipc-eval=") => {
            ensure_no_extra_args(args, 1)?;
            let json = arg.trim_start_matches("--ipc-eval=").to_owned();
            if json.is_empty() {
                return Err("--ipc-eval=<json> must not be empty".to_owned());
            }
            Ok(CliAction::IpcEval(json))
        }
        arg if is_launch_flag(arg) => parse_launch_args(args),
        other => Err(format!("unknown option `{other}`\n\n{}", usage_text())),
    }
}

fn is_launch_flag(arg: &str) -> bool {
    matches!(
        arg,
        "--url"
            | "--dev-url"
            | "--static-dir"
            | "--base-dir"
            | "--home-dir"
            | "--enable-control-server"
    ) || arg.starts_with("--url=")
        || arg.starts_with("--dev-url=")
        || arg.starts_with("--static-dir=")
        || arg.starts_with("--base-dir=")
        || arg.starts_with("--home-dir=")
}

fn parse_launch_args(args: &[String]) -> Result<CliAction, String> {
    let mut url: Option<String> = None;
    let mut dev_url: Option<String> = None;
    let mut static_dir: Option<PathBuf> = None;
    let mut base_dir: Option<PathBuf> = None;
    let mut enable_control_server = false;
    let mut idx = 0_usize;

    while idx < args.len() {
        let arg = args[idx].as_str();
        if arg == "--enable-control-server" {
            enable_control_server = true;
            idx += 1;
        } else if let Some(val) = arg.strip_prefix("--url=") {
            url = Some(non_empty_flag("--url", val)?);
            idx += 1;
        } else if let Some(val) = arg.strip_prefix("--dev-url=") {
            dev_url = Some(non_empty_flag("--dev-url", val)?);
            idx += 1;
        } else if let Some(val) = arg.strip_prefix("--static-dir=") {
            static_dir = Some(PathBuf::from(non_empty_flag("--static-dir", val)?));
            idx += 1;
        } else if let Some(val) = arg
            .strip_prefix("--base-dir=")
            .or_else(|| arg.strip_prefix("--home-dir="))
        {
            base_dir = Some(PathBuf::from(non_empty_flag("--home-dir", val)?));
            idx += 1;
        } else {
            let next = args
                .get(idx + 1)
                .ok_or_else(|| format!("missing value after `{arg}`"))?;
            match arg {
                "--url" => url = Some(non_empty_flag("--url", next)?),
                "--dev-url" => dev_url = Some(non_empty_flag("--dev-url", next)?),
                "--static-dir" => {
                    static_dir = Some(PathBuf::from(non_empty_flag("--static-dir", next)?));
                }
                "--base-dir" | "--home-dir" => {
                    base_dir = Some(PathBuf::from(non_empty_flag(arg, next)?));
                }
                other => return Err(format!("unknown --launch option `{other}`")),
            }
            idx += 2;
        }
    }

    Ok(CliAction::Launch {
        url,
        dev_url,
        static_dir,
        base_dir,
        enable_control_server,
    })
}

fn non_empty_flag(flag: &str, value: &str) -> Result<String, String> {
    if value.trim().is_empty() {
        return Err(format!("{flag} must not be empty"));
    }
    Ok(value.to_owned())
}

fn ensure_no_extra_args(args: &[String], expected: usize) -> Result<(), String> {
    if args.len() > expected {
        return Err(format!("unexpected extra argument `{}`", args[expected]));
    }
    Ok(())
}

fn usage_text() -> String {
    [
        "legioncode-desktop — Safe Rust Tauri v2 desktop host shell for LegionCode",
        "",
        "Usage:",
        "  legioncode-desktop [--home-dir <dir>] [--dev-url <url>] [--static-dir <dir>]",
        "  legioncode-desktop --version",
        "  legioncode-desktop --print-tauri-config",
        "  legioncode-desktop --emit-bundle <dir>",
        "  legioncode-desktop --ipc-eval <json>",
        "  legioncode-desktop --self-check",
        "  legioncode-desktop --smoke-webview | --headless-smoke",
        "  legioncode-desktop --launch [--url <url>] [--dev-url <url>] [--static-dir <dir>] [--home-dir <dir>]",
    ]
    .join("\n")
}

fn print_stdout(text: &str) -> u8 {
    let mut out = io::stdout().lock();
    if let Err(err) = writeln!(out, "{text}") {
        return fail(&format!("stdout write error: {err}"));
    }
    0
}

fn print_stdout_raw(text: &str) -> u8 {
    let mut out = io::stdout().lock();
    if let Err(err) = out.write_all(text.as_bytes()) {
        return fail(&format!("stdout write error: {err}"));
    }
    0
}

fn fail(message: &str) -> u8 {
    let _ = writeln!(io::stderr(), "{message}");
    1
}
