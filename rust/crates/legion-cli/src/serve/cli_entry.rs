//! Server lifecycle runner, interactive browser open, and Tailscale Serve attachment for
//! root `legioncode`, `legioncode start`, and `legioncode serve`.

use std::io::IsTerminal as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use legion_persistence::CancellationToken;
use legion_persistence::pairing_tokens::{
    ADMINISTRATIVE_CLIENT_SCOPES, IssuePairingTokenInput, issue_pairing_token,
};
use legion_persistence::paths::{ServerDerivedPaths, ensure_server_directories};
use legion_persistence::project_store::{CreateProjectInput, add_or_unarchive_project};
use legion_remote::live_network::LiveTailscaleController;
use legion_remote::tunnel::tailscale::ServeTarget;
use legion_shared::qr_code::build_pairing_url;

use super::cli_flags::{ServerEntryKind, parse_server_cli_args};
use super::config::ServeMode;
use super::{UNIT_SERVE, spawn_server_with_dev_url};
use crate::fallback::{delegate_to_node, unit_forced_original};

/// Runs `legioncode`, `legioncode start`, or `legioncode serve` natively without implicit Node delegation.
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

    let web_mode = parsed.config.mode == ServeMode::Web;
    let dev_url = parsed.dev_url;
    let allow_browser = kind != ServerEntryKind::Serve
        && web_mode
        && !parsed.config.no_browser
        && std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal();
    let tailscale_cfg = parsed
        .tailscale_serve
        .then_some(parsed.tailscale_serve_port);

    let server = match spawn_server_with_dev_url(parsed.config, dev_url.clone()).await {
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

    let pairing_url = if web_mode {
        let base_origin = dev_url.as_deref().unwrap_or_else(|| server.origin());
        mint_startup_pairing_url(&server.paths().db_path, base_origin)
    } else {
        None
    };

    println!("LegionCode server listening on {}", server.origin());
    if let Some(ref url) = pairing_url {
        println!("  pairingUrl: {url}");
    }
    if allow_browser {
        let target = pairing_url.as_deref().unwrap_or_else(|| server.origin());
        open_browser_best_effort(target);
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

fn mint_startup_pairing_url(db_path: &Path, origin: &str) -> Option<String> {
    const STARTUP_PAIRING_TTL_MS: i64 = 24 * 60 * 60 * 1_000;
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())?;
    let input = IssuePairingTokenInput::default()
        .with_scopes(ADMINISTRATIVE_CLIENT_SCOPES.iter().map(|s| (*s).to_owned()))
        .with_ttl_ms(STARTUP_PAIRING_TTL_MS)
        .with_label("Browser startup");
    let issued = issue_pairing_token(db_path, &input, now_ms).ok()?;
    build_pairing_url(origin, &issued.credential).ok()
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

pub(super) async fn wait_for_shutdown(fence: &CancellationToken) {
    #[cfg(unix)]
    {
        let mut sigterm =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            () = async {
                if let Some(ref mut term) = sigterm {
                    term.recv().await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => {}
            () = fence.cancelled() => {}
        }
    }
    #[cfg(not(unix))]
    {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            () = fence.cancelled() => {}
        }
    }
}
