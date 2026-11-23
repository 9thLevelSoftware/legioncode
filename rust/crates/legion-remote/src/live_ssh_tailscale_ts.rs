//! Live `tailscale` CLI subprocess execution and Serve controller.

use std::collections::HashSet;
use std::time::Duration;

use reqwest::Client;

use super::proc_io::{
    CliExecFailure, WellKnownProbe, build_no_redirect_http_client, probe_well_known_environment,
    run_cli_with_timeout, tcp_has_listener,
};
use super::types::map_serve_exit_error;
use crate::tunnel::tailscale::{
    ServeTarget, TAILSCALE_PROBE_TIMEOUT_MS, TAILSCALE_SERVE_TIMEOUT_MS,
    TAILSCALE_STATUS_TIMEOUT_MS, TailscaleError, TailscaleServeRootHolder,
    TailscaleServeRootStatus, TailscaleServeRoute, TailscaleStatus, build_tailscale_https_base_url,
    classify_serve_root_holder, classify_stderr_diagnostic, ensure_tailscale_serve_args,
    parse_tailscale_serve_root_holders, parse_tailscale_serve_routes_to, parse_tailscale_status,
    tailscale_command_for_platform, tailscale_serve_off_args,
};

/// Async CLI controller executing `tailscale status --json`, `serve status --json`, and `serve`.
///
/// ```
/// use legion_remote::live_network::LiveTailscaleController;
///
/// let ctl = LiveTailscaleController::new()?.with_executable("tailscale");
/// assert!(format!("{ctl:?}").contains("tailscale"));
/// # Ok::<(), legion_remote::tunnel::tailscale::TailscaleError>(())
/// ```
#[derive(Debug, Clone)]
pub struct LiveTailscaleController {
    executable: String,
    extra_env: Vec<(String, String)>,
    http: Client,
    probe_origin_override: Option<String>,
}

impl LiveTailscaleController {
    /// Creates a [`LiveTailscaleController`] for the host OS default `tailscale` binary.
    pub fn new() -> Result<Self, TailscaleError> {
        let http =
            build_no_redirect_http_client().map_err(|_| TailscaleError::TailscaleUnavailable)?;
        Ok(Self {
            executable: tailscale_command_for_platform(cfg!(windows)).to_owned(),
            extra_env: Vec::new(),
            http,
            probe_origin_override: None,
        })
    }

    /// Overrides the `tailscale` executable path.
    #[must_use]
    pub fn with_executable(mut self, executable: impl Into<String>) -> Self {
        self.executable = executable.into();
        self
    }

    /// Adds an environment variable passed to spawned `tailscale` invocations.
    #[must_use]
    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra_env.push((key.into(), value.into()));
        self
    }

    /// Overrides the base HTTP origin used to probe `/.well-known/legioncode/environment` in tests.
    #[must_use]
    pub fn with_probe_origin_override(mut self, origin: impl Into<String>) -> Self {
        self.probe_origin_override = Some(origin.into());
        self
    }

    /// Executes `tailscale status --json` and decodes [`TailscaleStatus`].
    pub async fn status(&self) -> Result<TailscaleStatus, TailscaleError> {
        let args = vec!["status".to_owned(), "--json".to_owned()];
        let out = run_cli_with_timeout(
            &self.executable,
            &args,
            &self.extra_env,
            Duration::from_millis(TAILSCALE_STATUS_TIMEOUT_MS),
        )
        .await
        .map_err(|_| TailscaleError::TailscaleUnavailable)?;
        if out.exit_code != Some(0) {
            return Err(TailscaleError::TailscaleUnavailable);
        }
        parse_tailscale_status(&out.stdout)
    }

    /// Executes `tailscale serve status --json` and parses routes targeting `target`.
    pub async fn serve_routes_to(
        &self,
        target: &ServeTarget,
    ) -> Result<Vec<TailscaleServeRoute>, TailscaleError> {
        let raw = self.read_serve_status_json().await?;
        parse_tailscale_serve_routes_to(&raw, target).ok_or_else(|| TailscaleError::StatusParse {
            detail: "unreadable serve status".to_owned(),
        })
    }

    /// Inspects who holds root `/` on `serve_port` (`None`, `Ours`, `Dead`, or `Other`).
    pub async fn serve_root_holder(
        &self,
        serve_port: u16,
        target: &ServeTarget,
    ) -> Result<TailscaleServeRootStatus, TailscaleError> {
        let raw = self.read_serve_status_json().await?;
        let holders =
            parse_tailscale_serve_root_holders(&raw, serve_port, target).ok_or_else(|| {
                TailscaleError::StatusParse {
                    detail: "unreadable serve status".to_owned(),
                }
            })?;
        let mut live_listeners = HashSet::new();
        for holder in &holders {
            if let TailscaleServeRootHolder::Other {
                loopback: Some(loopback),
            } = holder
                && tcp_has_listener(&loopback.host, loopback.port).await
            {
                live_listeners.insert((loopback.host.clone(), loopback.port));
            }
        }
        Ok(classify_serve_root_holder(&holders, |host, port| {
            live_listeners.contains(&(host.to_owned(), port))
        }))
    }

    /// Runs `tailscale serve --bg --https=<serve_port> http://<host>:<port>`, mapping CLI errors.
    pub async fn ensure_serve(
        &self,
        target: &ServeTarget,
        serve_port: u16,
    ) -> Result<(), TailscaleError> {
        if let Ok(status) = self.status().await
            && status.https_serve_enabled == Some(false)
        {
            return Err(TailscaleError::TailscaleServeNotEnabled);
        }
        let args = ensure_tailscale_serve_args(target.port, Some(serve_port), Some(&target.host));
        let out = run_cli_with_timeout(
            &self.executable,
            &args,
            &self.extra_env,
            Duration::from_millis(TAILSCALE_SERVE_TIMEOUT_MS),
        )
        .await
        .map_err(|failure| match failure {
            CliExecFailure::Spawn | CliExecFailure::Io | CliExecFailure::Timeout => {
                TailscaleError::TailscaleServeFailed { serve_port }
            }
        })?;

        if out.exit_code == Some(0) {
            return Ok(());
        }
        Err(map_serve_exit_error(
            classify_stderr_diagnostic(&out.stderr),
            serve_port,
            &target.host,
        ))
    }

    /// Removes the Serve web handler at `mount` (default `"/"`) on `serve_port`.
    pub async fn disable_serve(
        &self,
        serve_port: u16,
        mount: Option<&str>,
    ) -> Result<(), TailscaleError> {
        let route = TailscaleServeRoute {
            serve_port,
            mount: Some(mount.unwrap_or("/").to_owned()),
            host: None,
            ambiguous: None,
            owner: None,
        };
        let Some(args) = tailscale_serve_off_args(&route) else {
            return Ok(());
        };
        let out = run_cli_with_timeout(
            &self.executable,
            &args,
            &self.extra_env,
            Duration::from_millis(TAILSCALE_SERVE_TIMEOUT_MS),
        )
        .await
        .map_err(|_| TailscaleError::TailscaleServeFailed { serve_port })?;
        if out.exit_code == Some(0) {
            Ok(())
        } else {
            Err(map_serve_exit_error(
                classify_stderr_diagnostic(&out.stderr),
                serve_port,
                "127.0.0.1",
            ))
        }
    }

    /// Validates Tailscale status, checks root holder conflicts, and publishes `target` on `serve_port`.
    pub async fn publish_serve(
        &self,
        target: &ServeTarget,
        serve_port: u16,
        expected_environment_id: &str,
    ) -> Result<String, TailscaleError> {
        let status = self.status().await?;
        let magic_dns_name = status
            .magic_dns_name
            .ok_or(TailscaleError::MagicDnsNameMissing)?;
        if status.https_serve_enabled == Some(false) {
            return Err(TailscaleError::TailscaleServeNotEnabled);
        }
        let public_base_url = build_tailscale_https_base_url(&magic_dns_name, Some(serve_port))?;
        let probe_url = self
            .probe_origin_override
            .as_deref()
            .unwrap_or(&public_base_url);

        match probe_well_known_environment(
            &self.http,
            probe_url,
            Duration::from_millis(TAILSCALE_PROBE_TIMEOUT_MS),
        )
        .await
        {
            WellKnownProbe::LegionEnvironment(desc) => {
                if desc.environment_id.as_str() != expected_environment_id {
                    return Err(TailscaleError::ServesOtherEnvironment { serve_port });
                }
                return Ok(public_base_url);
            }
            WellKnownProbe::NotLegion(_) => {
                return Err(TailscaleError::ServePortOccupied { serve_port });
            }
            WellKnownProbe::Unreachable(_) => {}
        }

        if self.serve_root_holder(serve_port, target).await? == TailscaleServeRootStatus::Other {
            return Err(TailscaleError::ServePortOccupied { serve_port });
        }
        self.ensure_serve(target, serve_port).await?;
        Ok(public_base_url)
    }

    async fn read_serve_status_json(&self) -> Result<String, TailscaleError> {
        let args = vec!["serve".to_owned(), "status".to_owned(), "--json".to_owned()];
        let out = run_cli_with_timeout(
            &self.executable,
            &args,
            &self.extra_env,
            Duration::from_millis(TAILSCALE_SERVE_TIMEOUT_MS),
        )
        .await
        .map_err(|_| TailscaleError::TailscaleUnavailable)?;
        if out.exit_code != Some(0) {
            return Err(TailscaleError::TailscaleUnavailable);
        }
        Ok(out.stdout)
    }
}
