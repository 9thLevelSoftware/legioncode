#![forbid(unsafe_code)]
//! Modular Safe Rust `legioncode` Front-Door CLI and headless HTTP/WebSocket/MCP server.

mod boot_service_mgr;
mod boot_service_ops;
mod cli_global;
pub mod cmd_acp_mcp;
pub mod cmd_app;
pub mod cmd_auth;
mod cmd_auth_args;
mod cmd_auth_devices;
mod cmd_auth_live;
mod cmd_auth_ops;
pub mod cmd_browser;
#[cfg(target_os = "linux")]
mod cmd_browser_linux;
pub mod cmd_completions;
pub mod cmd_connect;
mod cmd_connect_ops;
pub mod cmd_doctor;
pub mod cmd_environment;
mod cmd_environment_ops;
pub mod cmd_feedback;
pub mod cmd_help;
mod cmd_help_catalog;
pub mod cmd_install;
pub mod cmd_pair;
mod cmd_pair_ts;
pub mod cmd_policy;
pub mod cmd_preflight;
pub mod cmd_project;
mod cmd_project_args;
mod cmd_project_http;
mod cmd_project_live;
pub mod cmd_service;
pub mod cmd_service_launcher;
pub mod cmd_speech_worker;
pub mod cmd_ssh_helper;
pub mod cmd_status;
pub mod cmd_status_human;
pub mod cmd_theme;
pub mod cmd_thread;
mod cmd_thread_args;
pub mod cmd_trace;
pub mod cmd_triage;
pub mod cmd_uninstall;
pub mod cmd_up;
pub mod cmd_update;
mod cmd_update_net;
pub mod cmd_workers;
pub mod cmd_workflow;
pub mod fallback;
pub mod invocation;
mod pipe_ipc;
pub mod serve;

use std::io::Write;

use cli_global::GlobalPrepass;
use fallback::{delegate_to_node, unit_forced_original};
use serve::cli_entry::run_entry;
use serve::cli_flags::ServerEntryKind;

/// Unit identifier for `RUST_ANYTHING_UNIT_VERSION` and fallback logs.
pub const UNIT_VERSION: &str = "VERSION";

/// Unit identifier for explicit `RUST_ANYTHING_UNIT_CLI_PASSTHROUGH=original` overrides.
pub const UNIT_CLI_PASSTHROUGH: &str = "CLI_PASSTHROUGH";

enum PreparedDispatch {
    Sync(i32),
    Serve {
        kind: ServerEntryKind,
        raw: Vec<String>,
        cleaned: Vec<String>,
    },
}

/// Synchronously runs fast-path CLI subcommands, entering a Tokio runtime only for `serve`/`start`/root.
pub fn run_cli_blocking(args: &[String]) -> i32 {
    match prepare_dispatch(args) {
        PreparedDispatch::Sync(code) => code,
        PreparedDispatch::Serve { kind, raw, cleaned } => {
            let Ok(runtime) = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
            else {
                eprintln!("Failed to initialize Tokio runtime for `legioncode serve`.");
                return 1;
            };
            runtime.block_on(run_entry(kind, &raw, &cleaned))
        }
    }
}

/// Executes the `legioncode` CLI dispatch pipeline given `args` (where `args[0]` is the executable path)
/// and returns the process exit code.
pub async fn run_cli(args: Vec<String>) -> i32 {
    match prepare_dispatch(&args) {
        PreparedDispatch::Sync(code) => code,
        PreparedDispatch::Serve { kind, raw, cleaned } => run_entry(kind, &raw, &cleaned).await,
    }
}

fn prepare_dispatch(args: &[String]) -> PreparedDispatch {
    let raw: &[String] = args.get(1..).unwrap_or(&[]);

    if let Some(code) = check_explicit_unit_kill_switch(raw) {
        return code;
    }

    let cleaned = match cli_global::run_global_prepass(raw) {
        GlobalPrepass::Exit(code) => return PreparedDispatch::Sync(code),
        GlobalPrepass::Continue { cleaned } => cleaned,
    };

    if cleaned.is_empty() {
        return PreparedDispatch::Serve {
            kind: ServerEntryKind::Root,
            raw: raw.to_vec(),
            cleaned,
        };
    }

    if cleaned.len() == 1 && (cleaned[0] == "--version" || cleaned[0] == "-v") {
        if unit_forced_original(UNIT_VERSION) {
            return PreparedDispatch::Sync(delegate_to_node(UNIT_VERSION, raw));
        }
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "legioncode v{}", env!("CARGO_PKG_VERSION"));
        return PreparedDispatch::Sync(0);
    }

    if let Some(code) = cmd_help::run_with_raw(&cleaned, raw) {
        return PreparedDispatch::Sync(code);
    }

    match cleaned[0].as_str() {
        "serve" => PreparedDispatch::Serve {
            kind: ServerEntryKind::Serve,
            raw: raw.to_vec(),
            cleaned,
        },
        "start" => PreparedDispatch::Serve {
            kind: ServerEntryKind::Start,
            raw: raw.to_vec(),
            cleaned,
        },
        _ => match run_known_sync_command(&cleaned) {
            Some(code) => PreparedDispatch::Sync(code),
            None => PreparedDispatch::Serve {
                kind: ServerEntryKind::Root,
                raw: raw.to_vec(),
                cleaned,
            },
        },
    }
}

fn check_explicit_unit_kill_switch(raw: &[String]) -> Option<PreparedDispatch> {
    let first = raw.first()?.as_str();
    match first {
        "serve" | "start" if unit_forced_original(serve::UNIT_SERVE) => {
            let kind = if first == "serve" {
                ServerEntryKind::Serve
            } else {
                ServerEntryKind::Start
            };
            return Some(PreparedDispatch::Serve {
                kind,
                raw: raw.to_vec(),
                cleaned: raw.to_vec(),
            });
        }
        "help" if unit_forced_original(cmd_help::UNIT_HELP) => {
            return cmd_help::run(raw).map(PreparedDispatch::Sync);
        }
        _ => {}
    }
    if let Some(unit) = unit_name_for_command(first)
        && unit_forced_original(unit)
    {
        return run_known_sync_command(raw).map(PreparedDispatch::Sync);
    }
    if unit_name_for_command(first).is_none()
        && !matches!(first, "serve" | "start" | "help")
        && !first.starts_with('-')
        && unit_forced_original(UNIT_CLI_PASSTHROUGH)
    {
        return Some(PreparedDispatch::Sync(delegate_to_node(
            UNIT_CLI_PASSTHROUGH,
            raw,
        )));
    }
    None
}

fn unit_name_for_command(cmd: &str) -> Option<&'static str> {
    Some(match cmd {
        "__service-preflight" => cmd_preflight::UNIT_SERVICE_PREFLIGHT,
        "__service-launcher" => cmd_service_launcher::UNIT_SERVICE_LAUNCHER,
        "__ssh-helper" => cmd_ssh_helper::UNIT_SSH_HELPER,
        "__speech-worker" => cmd_speech_worker::UNIT_SPEECH_WORKER,
        "__document-text" => cmd_workers::UNIT_DOCUMENT_TEXT,
        "__managed-chatgpt-token" => cmd_workers::UNIT_MANAGED_TOKEN,
        "__claude-history" => cmd_workers::UNIT_CLAUDE_HISTORY,
        "acp-mcp-bridge" | "acp-mcp-call" => cmd_acp_mcp::UNIT_ACP_MCP,
        "connect" => cmd_connect::UNIT_CONNECT,
        "pair" => cmd_pair::UNIT_PAIR,
        "thread" => cmd_thread::UNIT_THREAD,
        "status" => cmd_status::UNIT_STATUS,
        "policy" => cmd_policy::UNIT_POLICY,
        "doctor" => cmd_doctor::UNIT_DOCTOR,
        "project" => cmd_project::UNIT_PROJECT,
        "service" => cmd_service::UNIT_SERVICE,
        "trace" => cmd_trace::UNIT_TRACE,
        "theme" => cmd_theme::UNIT_THEME,
        "environment" => cmd_environment::UNIT_ENVIRONMENT,
        "auth" => cmd_auth::UNIT_AUTH,
        "up" => cmd_up::UNIT_UP,
        "triage" => cmd_triage::UNIT_TRIAGE,
        "app" => cmd_app::UNIT_APP,
        "browser" => cmd_browser::UNIT_BROWSER,
        "install" => cmd_install::UNIT_INSTALL,
        "uninstall" => cmd_uninstall::UNIT_UNINSTALL,
        "update" => cmd_update::UNIT_UPDATE,
        "workflow" => cmd_workflow::UNIT_WORKFLOW,
        "feedback" => cmd_feedback::UNIT_FEEDBACK,
        _ => return None,
    })
}

fn run_known_sync_command(args: &[String]) -> Option<i32> {
    let first = args.first()?.as_str();
    Some(match first {
        "__service-preflight" => cmd_preflight::run(args),
        "__service-launcher" => cmd_service_launcher::run(args),
        "__ssh-helper" => cmd_ssh_helper::run(args),
        "__speech-worker" => cmd_speech_worker::run(args),
        "__document-text" => cmd_workers::run_document_text(args),
        "__managed-chatgpt-token" => cmd_workers::run_managed_chatgpt_token(args),
        "__claude-history" => cmd_workers::run_claude_history(args),
        "acp-mcp-bridge" | "acp-mcp-call" => cmd_acp_mcp::run(args),
        "connect" => cmd_connect::run(args),
        "pair" => cmd_pair::run(args),
        "thread" => cmd_thread::run(args),
        "status" => cmd_status::run(args),
        "policy" => cmd_policy::run(args),
        "doctor" => cmd_doctor::run(args),
        "project" => cmd_project::run(args),
        "service" => cmd_service::run(args),
        "trace" => cmd_trace::run(args),
        "theme" => cmd_theme::run(args),
        "environment" => cmd_environment::run(args),
        "auth" => cmd_auth::run(args),
        "up" => cmd_up::run(args),
        "triage" => cmd_triage::run(args),
        "app" => cmd_app::run(args),
        "browser" => cmd_browser::run(args),
        "install" => cmd_install::run(args),
        "uninstall" => cmd_uninstall::run(args),
        "update" => cmd_update::run(args),
        "workflow" => cmd_workflow::run(args),
        "feedback" => cmd_feedback::run(args),
        _ => return None,
    })
}
