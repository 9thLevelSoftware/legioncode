//! Live provider CLI/HTTP probe service (`ProviderProbeService`) and speech discovery check.

use std::collections::BTreeMap;
use std::io::Read;
use std::time::Duration;

use legion_process_ffi::env::SanitizedChildEnv;
use legion_process_ffi::pipes::PipeSetConfig;
use legion_process_ffi::supervisor::{JobProcessSupervisor, SpawnSpec};
use legion_providers::drivers::antigravity::{
    AntigravityAuthConfig, AntigravityProbeOutcome, probe_antigravity,
};
use legion_providers::drivers::{ProviderProbeSpec, ProviderProbeStatus};
use legion_providers::non_acp::grok_http::build_grok_models_request;
use legion_speech::native_discovery::discover_speech_devices;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::ServerRuntimeError;

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
#[derive(Debug, Clone, Default)]
pub struct ProviderProbeService {
    http: reqwest::Client,
}

impl ProviderProbeService {
    /// Create a [`ProviderProbeService`] with a short probe HTTP timeout.
    pub fn new() -> Result<Self, ServerRuntimeError> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| ServerRuntimeError::Upstream(e.to_string()))?;
        Ok(Self { http })
    }

    /// Execute a probe against `spec` using [`JobProcessSupervisor`] and [`SanitizedChildEnv`].
    pub async fn probe_cli_spec(
        &self,
        spec: ProviderProbeSpec,
    ) -> Result<ProviderProbeResult, ServerRuntimeError> {
        tokio::task::spawn_blocking(move || run_cli_probe_blocking(&spec))
            .await
            .map_err(|e| ServerRuntimeError::Process(format!("probe task failed: {e}")))?
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
            } => self.probe_grok(endpoint_url.as_deref(), &api_key).await,
            ProviderProbeTarget::OpenCodeHttp {
                base_url,
                auth_header,
            } => self.probe_opencode(&base_url, auth_header.as_deref()).await,
            ProviderProbeTarget::Antigravity {
                instance_key,
                data_root,
                temp_root,
            } => {
                let auth = AntigravityAuthConfig::default();
                let outcome = probe_antigravity(None, &auth, &instance_key, &data_root, &temp_root);
                Ok(match outcome {
                    AntigravityProbeOutcome::NotInstalled { reason } => ProviderProbeResult {
                        status: ProviderProbeStatus::NotInstalled,
                        status_code: None,
                        message: reason,
                        details: json!({ "installed": false, "authenticated": false }),
                    },
                    AntigravityProbeOutcome::NeedsAuth { reason, target } => ProviderProbeResult {
                        status: ProviderProbeStatus::NeedsAuth,
                        status_code: None,
                        message: reason,
                        details: json!({
                            "installed": true,
                            "authenticated": false,
                            "executable": target.executable(),
                        }),
                    },
                    AntigravityProbeOutcome::Ready {
                        target,
                        directories,
                    } => ProviderProbeResult {
                        status: ProviderProbeStatus::Ready,
                        status_code: Some(0),
                        message: format!("Antigravity ready at {}", target.executable()),
                        details: json!({
                            "installed": true,
                            "authenticated": true,
                            "executable": target.executable(),
                            "geminiHome": directories.gemini_home,
                        }),
                    },
                })
            }
            ProviderProbeTarget::Speech => {
                let devices = discover_speech_devices();
                let count = devices.len();
                Ok(ProviderProbeResult {
                    status: ProviderProbeStatus::Ready,
                    status_code: Some(0),
                    message: format!("discovered {count} speech capture device(s)"),
                    details: serde_json::to_value(&devices)?,
                })
            }
        }
    }

    async fn probe_grok(
        &self,
        endpoint_url: Option<&str>,
        api_key: &str,
    ) -> Result<ProviderProbeResult, ServerRuntimeError> {
        let req_spec = build_grok_models_request(endpoint_url, api_key);
        let mut builder = self.http.get(&req_spec.url);
        for (k, v) in &req_spec.headers {
            builder = builder.header(k.as_str(), v.as_str());
        }
        match builder.send().await {
            Ok(resp) => {
                let code = i32::from(resp.status().as_u16());
                let status = if resp.status().is_success() {
                    ProviderProbeStatus::Ready
                } else if matches!(code, 401 | 403) {
                    ProviderProbeStatus::NeedsAuth
                } else {
                    ProviderProbeStatus::Error
                };
                Ok(ProviderProbeResult {
                    status,
                    status_code: Some(code),
                    message: format!("HTTP {code} from {}", req_spec.url),
                    details: json!({ "url": req_spec.url }),
                })
            }
            Err(err) => Ok(ProviderProbeResult {
                status: ProviderProbeStatus::Error,
                status_code: None,
                message: err.to_string(),
                details: json!({ "url": req_spec.url }),
            }),
        }
    }

    async fn probe_opencode(
        &self,
        base_url: &str,
        auth_header: Option<&str>,
    ) -> Result<ProviderProbeResult, ServerRuntimeError> {
        let trimmed = base_url.trim_end_matches('/');
        let url = format!("{trimmed}/global/health");
        let mut builder = self.http.get(&url);
        if let Some(auth) = auth_header {
            builder = builder.header("Authorization", auth);
        }
        match builder.send().await {
            Ok(resp) => {
                let code = i32::from(resp.status().as_u16());
                let status = if resp.status().is_success() {
                    ProviderProbeStatus::Ready
                } else if matches!(code, 401 | 403) {
                    ProviderProbeStatus::NeedsAuth
                } else {
                    ProviderProbeStatus::Error
                };
                Ok(ProviderProbeResult {
                    status,
                    status_code: Some(code),
                    message: format!("HTTP {code} from {url}"),
                    details: json!({ "url": url }),
                })
            }
            Err(err) => Ok(ProviderProbeResult {
                status: ProviderProbeStatus::Error,
                status_code: None,
                message: err.to_string(),
                details: json!({ "url": url }),
            }),
        }
    }
}

fn run_cli_probe_blocking(
    spec: &ProviderProbeSpec,
) -> Result<ProviderProbeResult, ServerRuntimeError> {
    let mut env = SanitizedChildEnv::from_os_env();
    for key in &spec.removed_env_keys {
        let _ = env.remove(key);
    }
    for (k, v) in &spec.env {
        env.set(k.as_str(), v.as_str());
    }
    let spawn_spec = SpawnSpec::new(&spec.executable)
        .args(&spec.args)
        .env(env)
        .stdio(PipeSetConfig::piped())
        .windows_no_window(true);

    let mut sup =
        JobProcessSupervisor::new().map_err(|e| ServerRuntimeError::Process(e.to_string()))?;
    sup.set_kill_on_drop(true);
    let Ok((token, mut pipes)) = sup.spawn(&spawn_spec) else {
        return Ok(ProviderProbeResult {
            status: ProviderProbeStatus::NotInstalled,
            status_code: None,
            message: format!("executable `{}` could not be spawned", spec.executable),
            details: json!({ "executable": spec.executable }),
        });
    };
    let _ = pipes.take_stdin();
    let mut stdout_str = String::new();
    let mut stderr_str = String::new();
    if let Some(mut out) = pipes.take_stdout() {
        let _ = out.read_to_string(&mut stdout_str);
    }
    if let Some(mut err) = pipes.take_stderr() {
        let _ = err.read_to_string(&mut stderr_str);
    }
    let exit = sup
        .wait(token)
        .map_err(|e| ServerRuntimeError::Process(e.to_string()))?;
    let code = exit.code();
    let combined = format!("{stdout_str}\n{stderr_str}").trim().to_owned();
    let lower = combined.to_ascii_lowercase();
    let status = if exit.success() {
        ProviderProbeStatus::Ready
    } else if lower.contains("unauthorized")
        || lower.contains("not logged in")
        || lower.contains("login")
    {
        ProviderProbeStatus::NeedsAuth
    } else {
        ProviderProbeStatus::Error
    };
    Ok(ProviderProbeResult {
        status,
        status_code: code,
        message: combined.lines().next().unwrap_or("probe exited").to_owned(),
        details: json!({ "stdout": stdout_str.trim(), "stderr": stderr_str.trim() }),
    })
}
