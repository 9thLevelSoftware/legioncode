//! Async subprocess execution, bounded stderr capture, and `/.well-known/legioncode/environment` probes.

use std::process::Stdio;
use std::time::Duration;

use legion_contracts::environment::ExecutionEnvironmentDescriptor;
use reqwest::Client;
use reqwest::header::ACCEPT;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::tunnel::ssh::{append_stderr_tail, redact_ssh_error_output};

/// Result of running a short-lived CLI subprocess under a timeout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CliOutput {
    pub(super) exit_code: Option<i32>,
    pub(super) stdout: String,
    pub(super) stderr: String,
}

/// Failure executing a short-lived CLI subprocess.
#[derive(Debug)]
pub(super) enum CliExecFailure {
    Spawn,
    Timeout,
    Io,
}

/// Result of probing `<origin>/.well-known/legioncode/environment`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum WellKnownProbe {
    LegionEnvironment(Box<ExecutionEnvironmentDescriptor>),
    NotLegion(u16),
    Unreachable(String),
}

/// Appends redacted `bytes` to `tail`, preserving at most `TUNNEL_STDERR_TAIL_CHARS`.
pub(super) fn push_redacted_stderr_tail(tail: &mut String, bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    let decoded = String::from_utf8_lossy(bytes);
    let redacted = redact_ssh_error_output(&decoded, true);
    append_stderr_tail(tail, &redacted);
}

/// Runs `executable` with `args` cleanly draining both stdout and stderr under `timeout`.
pub(super) async fn run_cli_with_timeout(
    executable: &str,
    args: &[String],
    extra_env: &[(String, String)],
    timeout: Duration,
) -> Result<CliOutput, CliExecFailure> {
    let mut cmd = Command::new(executable);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for (key, val) in extra_env {
        cmd.env(key, val);
    }

    let mut child = cmd.spawn().map_err(|_| CliExecFailure::Spawn)?;
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();

    let run_fut = async {
        let read_stdout = async move {
            let mut buf = Vec::new();
            if let Some(mut pipe) = stdout_pipe {
                pipe.read_to_end(&mut buf).await?;
            }
            Ok::<Vec<u8>, std::io::Error>(buf)
        };
        let read_stderr = async move {
            let mut buf = Vec::new();
            if let Some(mut pipe) = stderr_pipe {
                pipe.read_to_end(&mut buf).await?;
            }
            Ok::<Vec<u8>, std::io::Error>(buf)
        };
        let (status_res, stdout_res, stderr_res) =
            tokio::join!(child.wait(), read_stdout, read_stderr);
        let status = status_res.map_err(|_| CliExecFailure::Io)?;
        let stdout_bytes = stdout_res.map_err(|_| CliExecFailure::Io)?;
        let stderr_bytes = stderr_res.map_err(|_| CliExecFailure::Io)?;
        Ok(CliOutput {
            exit_code: status.code(),
            stdout: String::from_utf8_lossy(&stdout_bytes).into_owned(),
            stderr: String::from_utf8_lossy(&stderr_bytes).into_owned(),
        })
    };

    if let Ok(done) = tokio::time::timeout(timeout, run_fut).await {
        done
    } else {
        let _ = child.start_kill();
        let _ = child.wait().await;
        Err(CliExecFailure::Timeout)
    }
}

/// Probes `<base_url>/.well-known/legioncode/environment` classifying `LegionCode` vs non-`LegionCode` vs unreachable hosts.
pub(super) async fn probe_well_known_environment(
    http: &Client,
    base_url: &str,
    timeout: Duration,
) -> WellKnownProbe {
    let trimmed = base_url.trim_end_matches('/');
    let url = format!("{trimmed}/.well-known/legioncode/environment");
    let resp = match http
        .get(&url)
        .header(ACCEPT, "application/json")
        .timeout(timeout)
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(err) => return WellKnownProbe::Unreachable(err.to_string()),
    };

    let status = resp.status().as_u16();
    if matches!(status, 502..=504) {
        return WellKnownProbe::Unreachable(format!("gateway HTTP {status}"));
    }
    if !resp.status().is_success() {
        return WellKnownProbe::NotLegion(status);
    }

    let Ok(bytes) = resp.bytes().await else {
        return WellKnownProbe::Unreachable("failed to read descriptor body".to_owned());
    };
    match serde_json::from_slice::<ExecutionEnvironmentDescriptor>(&bytes) {
        Ok(descriptor) => WellKnownProbe::LegionEnvironment(Box::new(descriptor)),
        Err(_) => WellKnownProbe::NotLegion(status),
    }
}

/// Checks asynchronously whether a TCP listener answers on `host:port`.
pub(super) async fn tcp_has_listener(host: &str, port: u16) -> bool {
    let clean_host = host.trim_start_matches('[').trim_end_matches(']');
    matches!(
        tokio::time::timeout(
            Duration::from_millis(350),
            tokio::net::TcpStream::connect((clean_host, port)),
        )
        .await,
        Ok(Ok(_))
    )
}

/// Builds a `reqwest::Client` that refuses redirects so pairing tokens/descriptors never leak.
pub(super) fn build_no_redirect_http_client() -> Result<Client, reqwest::Error> {
    Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
}
