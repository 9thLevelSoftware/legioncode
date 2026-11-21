//! Server lifecycle runner, interactive browser open, and Tailscale Serve attachment for
//! root `t3`, `t3 start`, and `t3 serve`.

use std::io::IsTerminal as _;
use std::process::{Command, Stdio};

use t3_persistence::paths::{ServerDerivedPaths, ensure_server_directories};
use t3_persistence::project_store::{CreateProjectInput, add_or_unarchive_project};
use t3_remote::live_network::LiveTailscaleController;
use t3_remote::tunnel::tailscale::ServeTarget;

use super::cli_flags::{ServerEntryKind, parse_server_cli_args};
use super::config::ServeMode;
use super::{UNIT_SERVE, spawn_server, wait_for_shutdown};
use crate::fallback::{delegate_to_node, unit_forced_original};

/// Runs `t3`, `t3 start`, or `t3 serve` natively without implicit Node delegation.
pub(crate) async fn run_entry(
    kind: ServerEntryKind,
    raw_args: &[String],
    cleaned_args: &[String],
) -> i32 {
    if unit_forced_original(UNIT_SERVE) {
        return delegate_to_node(UNIT_SERVE, raw_args);
    }

    let tail = match kind {
        ServerEntryKind::Root => cleaned_args,
        ServerEntryKind::Start | ServerEntryKind::Serve => cleaned_args.get(1..).unwrap_or(&[]),
    };
    let parsed = match parse_server_cli_args(kind, tail) {
        Ok(parsed) => parsed,
        Err(code) => return code,
    };

    if parsed.auto_bootstrap && parsed.config.cwd.is_dir() {
        let paths = ServerDerivedPaths::derive(
            &parsed.config.base_dir,
            false,
            parsed.config.base_dir_is_explicit,
        );
        if ensure_server_directories(&paths).is_ok() {
            let root = parsed.config.cwd.to_string_lossy().into_owned();
            let _ = add_or_unarchive_project(&paths.db_path, &CreateProjectInput::new(root));
        }
    }

    let allow_browser = kind != ServerEntryKind::Serve
        && parsed.config.mode == ServeMode::Web
        && !parsed.config.no_browser
        && std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal();
    let tailscale_cfg = parsed
        .tailscale_serve
        .then_some(parsed.tailscale_serve_port);

    let server = match spawn_server(parsed.config).await {
        Ok(srv) => srv,
        Err(err) => {
            eprintln!("{err}");
            return err.exit_code();
        }
    };

    let tailscale_published = if let Some(serve_port) = tailscale_cfg {
        configure_tailscale_serve(
            server.addr.ip().to_string(),
            server.addr.port(),
            serve_port,
            server.environment_id.as_str(),
        )
        .await
    } else {
        false
    };

    println!("T3 Code server listening on {}", server.origin());
    if allow_browser {
        open_browser_best_effort(server.origin());
    }

    wait_for_shutdown(&server.lock_fence()).await;
    if tailscale_published && let Some(serve_port) = tailscale_cfg {
        disable_tailscale_serve(serve_port).await;
    }

    if let Err(err) = server.shutdown().await {
        eprintln!("Error shutting down server: {err}");
        return 1;
    }
    0
}

async fn configure_tailscale_serve(
    bind_ip: String,
    local_port: u16,
    serve_port: u16,
    environment_id: &str,
) -> bool {
    let Ok(ctl) = LiveTailscaleController::new() else {
        eprintln!("Warning: Tailscale Serve controller could not be initialized.");
        return false;
    };
    let target_host = if bind_ip == "0.0.0.0" || bind_ip == "::" {
        "127.0.0.1".to_owned()
    } else {
        bind_ip
    };
    let target = ServeTarget {
        host: target_host,
        port: local_port,
    };
    match ctl.publish_serve(&target, serve_port, environment_id).await {
        Ok(url) => {
            println!("Tailscale Serve active at {url}");
            true
        }
        Err(err) => {
            eprintln!("Warning: Failed to configure Tailscale Serve: {err}");
            false
        }
    }
}

async fn disable_tailscale_serve(serve_port: u16) {
    if let Ok(ctl) = LiveTailscaleController::new() {
        let _ = ctl.disable_serve(serve_port, None).await;
    }
}

fn open_browser_best_effort(url: &str) {
    #[cfg(windows)]
    let mut cmd = {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", "", url]);
        c
    };
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = Command::new("open");
        c.arg(url);
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = {
        let mut c = Command::new("xdg-open");
        c.arg(url);
        c
    };
    let _ = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}
