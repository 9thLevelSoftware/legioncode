//! Live provider CLI/HTTP probe service (`ProviderProbeService`) and speech discovery check.

use std::collections::BTreeMap;
use std::io::Read;
use std::time::Duration;

use futures_util::future::join_all;
use legion_process_ffi::env::SanitizedChildEnv;
use legion_process_ffi::pipes::PipeSetConfig;
use legion_process_ffi::supervisor::{JobProcessSupervisor, SpawnSpec};
use legion_protocol::provider_env::SanitizedEnv;
use legion_providers::core::{
    ProviderDiscoveryOutcome, ServerProviderSnapshot, build_server_provider_snapshot,
};
use legion_providers::drivers::{
    DISCOVERY_CLI_DRIVERS, ProviderProbeSpec, ProviderProbeStatus, builtin_cli_probe_spec,
    classify_cli_probe_output, find_builtin_driver_spec, resolve_cli_probe_command,
    summarize_antigravity_probe,
};
use legion_providers::non_acp::grok_http::build_grok_models_request;
use legion_speech::native_discovery::discover_speech_devices;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::ServerRuntimeError;
use crate::outbox_state::utc_now_iso;

/// Default maximum time a single CLI discovery probe (`--version`) may run before termination.
pub const DEFAULT_CLI_PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// Probe target specification accepted by [`ProviderProbeService`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ProviderProbeTarget {
    /// Execute a CLI binary probe from [`ProviderProbeSpec`].
    #[serde(rename_all = "camelCase")]
    Cli {
        /// Program executable path or name.
        executable: String,
        /// Probe arguments (e.g. `["--version"]`).
        #[serde(default)]
        args: Vec<String>,
        /// Extra environment variables.
        #[serde(default)]
        env: BTreeMap<String, String>,
        /// Sensitive environment variable keys to scrub (`BUG-09`).
        #[serde(default)]
        removed_env_keys: Vec<String>,
    },
    /// Probe Grok HTTP `/models` endpoint with an API key.
    #[serde(rename_all = "camelCase")]
    GrokHttp {
        /// Optional endpoint override (`None` uses default xAI base URL).
        #[serde(default)]
        endpoint_url: Option<String>,
        /// Bearer API key.
        api_key: String,
    },
    /// Probe `OpenCode` HTTP server health endpoint.
    #[serde(rename_all = "camelCase")]
    OpenCodeHttp {
        /// Base URL of the `OpenCode` server.
        base_url: String,
        /// Optional `Authorization` header override.
        #[serde(default)]
        auth_header: Option<String>,
    },
    /// Probe local Antigravity installation/auth state.
    #[serde(rename_all = "camelCase")]
    Antigravity {
        /// Instance key.
        instance_key: String,
        /// Data directory root.
        data_root: String,
        /// Temp directory root.
        temp_root: String,
    },
    /// Probe local native speech/ASR hardware capabilities via `legion-speech`.
    Speech,
}

/// Outcome returned by [`ProviderProbeService::run_probe`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderProbeResult {
    /// High-level availability classification.
    pub status: ProviderProbeStatus,
    /// Optional subprocess exit code or HTTP status code.
    pub status_code: Option<i32>,
    /// Concise diagnostic message.
    pub message: String,
    /// Structured extra details.
    pub details: Value,
}

/// Executes live provider CLI/HTTP/speech availability probes under `BUG-09` isolation.
#[derive(Debug, Clone)]
pub struct ProviderProbeService {
    http: reqwest::Client,
    cli_timeout: Duration,
}

impl Default for ProviderProbeService {
    fn default() -> Self {
        Self {
            http: reqwest::Client::new(),
            cli_timeout: DEFAULT_CLI_PROBE_TIMEOUT,
        }
    }
}

impl ProviderProbeService {
    /// Create a [`ProviderProbeService`] with short HTTP and CLI probe timeouts.
    pub fn new() -> Result<Self, ServerRuntimeError> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| ServerRuntimeError::Upstream(e.to_string()))?;
        Ok(Self {
            http,
            cli_timeout: DEFAULT_CLI_PROBE_TIMEOUT,
        })
    }

    /// Overrides the per-CLI probe execution timeout before forced process-tree termination.
    #[must_use]
    pub const fn with_cli_timeout(mut self, timeout: Duration) -> Self {
        self.cli_timeout = timeout;
        self
    }

    /// Execute a probe against `spec` using [`JobProcessSupervisor`] and bounded timeout.
    pub async fn probe_cli_spec(
        &self,
        spec: ProviderProbeSpec,
    ) -> Result<ProviderProbeResult, ServerRuntimeError> {
        let timeout = self.cli_timeout;
        tokio::task::spawn_blocking(move || run_cli_probe_blocking(&spec, timeout))
            .await
            .map_err(|e| ServerRuntimeError::Process(format!("probe task failed: {e}")))?
    }

    /// Probes one built-in driver (or `"devReplay"`) and returns a contract-aligned [`ServerProviderSnapshot`].
    pub async fn probe_driver_snapshot(
        &self,
        driver_or_alias: &str,
        checked_at: &str,
    ) -> Result<ServerProviderSnapshot, ServerRuntimeError> {
        let spec = find_builtin_driver_spec(driver_or_alias).ok_or_else(|| {
            ServerRuntimeError::NotFound(format!("unknown provider driver `{driver_or_alias}`"))
        })?;
        if spec.slug == "devReplay" {
            return Ok(build_server_provider_snapshot(
                spec,
                ProviderDiscoveryOutcome {
                    instance_id: spec.slug.to_owned(),
                    probe_status: ProviderProbeStatus::Ready,
                    version: Some("builtin".to_owned()),
                    checked_at: checked_at.to_owned(),
                    message: Some("Built-in Replay (dev) driver ready".to_owned()),
                },
            ));
        }
        let sanitized = SanitizedEnv::from_entries(std::env::vars(), cfg!(windows));
        let probe_spec = builtin_cli_probe_spec(&sanitized, spec.slug).ok_or_else(|| {
            ServerRuntimeError::NotFound(format!("no CLI probe defined for `{}`", spec.slug))
        })?;
        let res = self.probe_cli_spec(probe_spec).await?;
        let version = res
            .details
            .get("version")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        Ok(build_server_provider_snapshot(
            spec,
            ProviderDiscoveryOutcome {
                instance_id: spec.slug.to_owned(),
                probe_status: res.status,
                version,
                checked_at: checked_at.to_owned(),
                message: (!res.message.is_empty()).then_some(res.message),
            },
        ))
    }

    /// Concurrently discovers local CLI providers (`codex`, `claudeAgent`, `cursor`, `opencode`,
    /// `pi`, `kiro`, `acpRegistry`) plus built-in `devReplay` using bounded non-hanging timeouts.
    pub async fn discover_local_providers(&self) -> Vec<ServerProviderSnapshot> {
        let checked_at = utc_now_iso();
        let futs = DISCOVERY_CLI_DRIVERS
            .iter()
            .copied()
            .chain(std::iter::once("devReplay"))
            .map(|slug| self.probe_driver_snapshot(slug, &checked_at));
        join_all(futs).await.into_iter().flatten().collect()
    }

    /// Dispatch a typed [`ProviderProbeTarget`] probe.
    pub async fn run_probe(
        &self,
        target: ProviderProbeTarget,
    ) -> Result<ProviderProbeResult, ServerRuntimeError> {
        match target {
            ProviderProbeTarget::Cli {
                executable,
                args,
                env,
                removed_env_keys,
            } => {
                self.probe_cli_spec(ProviderProbeSpec {
                    executable,
                    args,
                    env,
                    removed_env_keys,
                })
                .await
            }
            ProviderProbeTarget::GrokHttp {
                endpoint_url,
                api_key,
            } => {
                let req = build_grok_models_request(endpoint_url.as_deref(), &api_key);
                let mut builder = self.http.get(&req.url);
                for (k, v) in &req.headers {
                    builder = builder.header(k.as_str(), v.as_str());
                }
                Ok(map_http_probe(builder.send().await, &req.url))
            }
            ProviderProbeTarget::OpenCodeHttp {
                base_url,
                auth_header,
            } => {
                let url = format!("{}/global/health", base_url.trim_end_matches('/'));
                let mut builder = self.http.get(&url);
                if let Some(auth) = auth_header {
                    builder = builder.header("Authorization", auth);
                }
                Ok(map_http_probe(builder.send().await, &url))
            }
            ProviderProbeTarget::Antigravity {
                instance_key,
                data_root,
                temp_root,
            } => {
                let (status, code, msg, details) =
                    summarize_antigravity_probe(&instance_key, &data_root, &temp_root);
                Ok(probe_res(status, code, msg, details))
            }
            ProviderProbeTarget::Speech => {
                let devices = discover_speech_devices();
                let count = devices.len();
                Ok(probe_res(
                    ProviderProbeStatus::Ready,
                    Some(0),
                    format!("discovered {count} speech capture device(s)"),
                    serde_json::to_value(&devices)?,
                ))
            }
        }
    }
}

fn probe_res(
    status: ProviderProbeStatus,
    status_code: Option<i32>,
    message: impl Into<String>,
    details: Value,
) -> ProviderProbeResult {
    ProviderProbeResult {
        status,
        status_code,
        message: message.into(),
        details,
    }
}

fn map_http_probe(
    resp: Result<reqwest::Response, reqwest::Error>,
    url: &str,
) -> ProviderProbeResult {
    match resp {
        Ok(ok) => {
            let code = i32::from(ok.status().as_u16());
            let status = if ok.status().is_success() {
                ProviderProbeStatus::Ready
            } else if matches!(code, 401 | 403) {
                ProviderProbeStatus::NeedsAuth
            } else {
                ProviderProbeStatus::Error
            };
            probe_res(
                status,
                Some(code),
                format!("HTTP {code} from {url}"),
                json!({ "url": url }),
            )
        }
        Err(err) => probe_res(
            ProviderProbeStatus::Error,
            None,
            err.to_string(),
            json!({ "url": url }),
        ),
    }
}

fn run_cli_probe_blocking(
    spec: &ProviderProbeSpec,
    timeout: Duration,
) -> Result<ProviderProbeResult, ServerRuntimeError> {
    let mut env = SanitizedChildEnv::from_os_env();
    for key in &spec.removed_env_keys {
        let _ = env.remove(key);
    }
    for (k, v) in &spec.env {
        env.set(k.as_str(), v.as_str());
    }
    let Some((launch_exe, launch_args)) = resolve_cli_probe_command(
        &spec.executable,
        &spec.args,
        env.get("PATH"),
        env.get("PATHEXT"),
        cfg!(windows),
    ) else {
        return Ok(probe_res(
            ProviderProbeStatus::NotInstalled,
            None,
            format!("executable `{}` was not found on PATH", spec.executable),
            json!({ "executable": spec.executable }),
        ));
    };

    let spawn_spec = SpawnSpec::new(&launch_exe)
        .args(&launch_args)
        .env(env)
        .stdio(PipeSetConfig::piped())
        .windows_no_window(true);
    let mut sup =
        JobProcessSupervisor::new().map_err(|e| ServerRuntimeError::Process(e.to_string()))?;
    sup.set_kill_on_drop(true);
    let Ok((token, mut pipes)) = sup.spawn(&spawn_spec) else {
        return Ok(probe_res(
            ProviderProbeStatus::NotInstalled,
            None,
            format!("executable `{}` could not be spawned", spec.executable),
            json!({ "executable": spec.executable }),
        ));
    };
    let _ = pipes.take_stdin();
    let stdout_task = spawn_pipe_reader(pipes.take_stdout());
    let stderr_task = spawn_pipe_reader(pipes.take_stderr());

    let Some(exit) = sup
        .wait_timeout(token, timeout)
        .map_err(|e| ServerRuntimeError::Process(e.to_string()))?
    else {
        let _ = sup.terminate_child(token);
        let _ = sup.terminate_tree(1);
        return Ok(probe_res(
            ProviderProbeStatus::Error,
            None,
            format!(
                "timed out after {}ms probing `{}`",
                timeout.as_millis(),
                spec.executable
            ),
            json!({
                "timedOut": true,
                "stdout": stdout_task.join().unwrap_or_default().trim(),
                "stderr": stderr_task.join().unwrap_or_default().trim(),
            }),
        ));
    };

    let stdout_str = stdout_task.join().unwrap_or_default();
    let stderr_str = stderr_task.join().unwrap_or_default();
    let (status, version, summary) =
        classify_cli_probe_output(&stdout_str, &stderr_str, exit.success());
    Ok(probe_res(
        status,
        exit.code(),
        summary,
        json!({ "version": version, "stdout": stdout_str.trim(), "stderr": stderr_str.trim() }),
    ))
}

fn spawn_pipe_reader(pipe: Option<std::io::PipeReader>) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut buf = String::new();
        if let Some(mut reader) = pipe {
            let _ = reader.read_to_string(&mut buf);
        }
        buf
    })
}
