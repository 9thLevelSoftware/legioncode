//! Safe async `tokio::process` CLI runner (`env_clear` + `stdin`) and live `git` workspace ops.

use std::path::Path;
use std::process::Stdio;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::core::discovery::{ParsedRemote, parse_git_remote_url};
use crate::forges::CliCommandSpec;
pub use crate::live_git_perf::compute_live_git_diff;
use crate::live_http::LiveClientError;

const SAFE_BASE_ENV_KEYS: &[&str] = &[
    "PATH",
    "SYSTEMROOT",
    "WINDIR",
    "COMSPEC",
    "PATHEXT",
    "HOME",
    "USERPROFILE",
    "HOMEDRIVE",
    "HOMEPATH",
    "TMP",
    "TEMP",
    "TMPDIR",
    "LOCALAPPDATA",
    "APPDATA",
    "XDG_CONFIG_HOME",
    "LANG",
    "LC_ALL",
];

/// Captured process output from executing a [`CliCommandSpec`] or `git` invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveCliOutput {
    /// Exit code returned by the child process, if terminated normally.
    pub status_code: Option<i32>,
    /// Whether exit status reported success (`0`).
    pub success: bool,
    /// Decoded `stdout` string (`UTF-8` lossy).
    pub stdout: String,
    /// Decoded `stderr` string (`UTF-8` lossy).
    pub stderr: String,
}

/// Collects a minimal non-secret subset of environment variables for child processes.
#[must_use]
pub fn collect_safe_base_env() -> Vec<(String, String)> {
    SAFE_BASE_ENV_KEYS
        .iter()
        .filter_map(|&key| std::env::var(key).ok().map(|val| (key.to_owned(), val)))
        .collect()
}

/// Executes `spec` using `tokio::process::Command` with `env_clear()`, explicit env vars,
/// optional `working_dir`, and optional piped `stdin` (`@-` delivery).
pub async fn execute_cli_spec(
    spec: &CliCommandSpec,
    base_env: &[(String, String)],
    working_dir: Option<&Path>,
) -> Result<LiveCliOutput, LiveClientError> {
    let raw = execute_raw_bytes(
        &spec.program,
        &spec.args,
        base_env,
        &spec.env,
        spec.stdin.as_deref(),
        working_dir,
    )
    .await?;
    Ok(LiveCliOutput {
        status_code: raw.status_code,
        success: raw.success,
        stdout: String::from_utf8_lossy(&raw.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&raw.stderr).into_owned(),
    })
}

/// Runs `git remote get-url` inside `repo_dir` and parses the remote via [`parse_git_remote_url`].
pub async fn discover_workspace_remote(
    repo_dir: &Path,
    remote_name: Option<&str>,
    base_env: &[(String, String)],
) -> Result<ParsedRemote, LiveClientError> {
    let remote = remote_name.unwrap_or("origin").trim();
    if remote.is_empty() || remote.starts_with('-') {
        return Err(LiveClientError::InvalidInput(format!(
            "invalid git remote name `{remote}`"
        )));
    }
    let spec = CliCommandSpec {
        program: "git".to_owned(),
        args: vec!["remote".to_owned(), "get-url".to_owned(), remote.to_owned()],
        env: Vec::new(),
        stdin: None,
    };
    let out = execute_cli_spec(&spec, base_env, Some(repo_dir)).await?;
    if !out.success {
        return Err(LiveClientError::CliExit {
            program: "git".to_owned(),
            code: out.status_code,
            stderr: out.stderr.trim().to_owned(),
        });
    }
    Ok(parse_git_remote_url(out.stdout.trim())?)
}

pub(crate) struct RawProcessOutput {
    pub status_code: Option<i32>,
    pub success: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

pub(crate) async fn execute_raw_bytes(
    program: &str,
    args: &[String],
    base_env: &[(String, String)],
    override_env: &[(String, String)],
    stdin_payload: Option<&str>,
    working_dir: Option<&Path>,
) -> Result<RawProcessOutput, LiveClientError> {
    let mut cmd = Command::new(program);
    cmd.env_clear();
    for (k, v) in base_env {
        cmd.env(k, v);
    }
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    for (k, v) in override_env {
        cmd.env(k, v);
    }
    cmd.args(args);
    if let Some(dir) = working_dir {
        cmd.current_dir(dir);
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    cmd.stdin(if stdin_payload.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });

    let mut child = cmd.spawn().map_err(|e| cli_io_err(program, &e))?;
    if let Some(payload) = stdin_payload
        && let Some(mut stdin_pipe) = child.stdin.take()
    {
        stdin_pipe
            .write_all(payload.as_bytes())
            .await
            .map_err(|e| cli_io_err(program, &e))?;
        stdin_pipe
            .shutdown()
            .await
            .map_err(|e| cli_io_err(program, &e))?;
    }

    let output = child
        .wait_with_output()
        .await
        .map_err(|e| cli_io_err(program, &e))?;

    Ok(RawProcessOutput {
        status_code: output.status.code(),
        success: output.status.success(),
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

pub(crate) fn cli_io_err(program: &str, err: &std::io::Error) -> LiveClientError {
    LiveClientError::Cli {
        program: program.to_owned(),
        reason: err.to_string(),
    }
}
