//! Safe async `tokio::process` CLI runner (`env_clear` + `stdin`) and live `git` workspace ops.

use std::path::Path;
use std::process::Stdio;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::checkpoints::diff_query::{
    CheckpointDiffQuery, ComputedDiff, DiffFileInput, DiffQueryOptions,
};
use crate::core::discovery::{ParsedRemote, parse_git_remote_url};
use crate::forges::CliCommandSpec;
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

/// Computes a unified diff against `base_ref` in a real `git` repository via [`CheckpointDiffQuery`].
///
/// Compares tracked changes (`git diff --no-renames -z --name-status <base_ref>`) and
/// untracked non-ignored files (`git ls-files --others --exclude-standard -z`) against `base_ref`.
pub async fn compute_live_git_diff(
    repo_dir: &Path,
    base_ref: &str,
    opts: &DiffQueryOptions,
    base_env: &[(String, String)],
) -> Result<ComputedDiff, LiveClientError> {
    let clean_ref = base_ref.trim();
    if clean_ref.is_empty() || clean_ref.starts_with('-') || clean_ref.contains('\0') {
        return Err(LiveClientError::InvalidInput(format!(
            "unsafe git base_ref `{base_ref}`"
        )));
    }

    let diff_raw = execute_git_checked(
        repo_dir,
        &["diff", "--no-renames", "-z", "--name-status", clean_ref],
        base_env,
    )
    .await?;
    let mut tracked_entries = parse_name_status_z(&diff_raw);

    let untracked_raw = execute_git_checked(
        repo_dir,
        &["ls-files", "--others", "--exclude-standard", "-z"],
        base_env,
    )
    .await?;
    for slice in untracked_raw.split(|&b| b == 0) {
        if !slice.is_empty()
            && let Ok(path) = std::str::from_utf8(slice)
            && !tracked_entries.iter().any(|(_, p)| p == path)
        {
            tracked_entries.push(('A', path.to_owned()));
        }
    }

    tracked_entries.sort_by(|a, b| a.1.cmp(&b.1));

    let mut buffers: Vec<OwnedDiffPair> = Vec::with_capacity(tracked_entries.len());
    for (status, path) in tracked_entries {
        let old_bytes = if status == 'A' {
            Vec::new()
        } else {
            read_git_blob(repo_dir, clean_ref, &path, base_env).await?
        };
        let new_bytes = if status == 'D' {
            Vec::new()
        } else {
            read_working_file_or_empty(&repo_dir.join(&path)).await?
        };
        buffers.push(OwnedDiffPair::from_bytes(path, old_bytes, new_bytes));
    }

    let inputs: Vec<DiffFileInput<'_>> = buffers.iter().map(OwnedDiffPair::as_input).collect();
    Ok(CheckpointDiffQuery::compute_files(&inputs, *opts))
}

async fn read_working_file_or_empty(path: &Path) -> Result<Vec<u8>, LiveClientError> {
    match tokio::fs::read(path).await {
        Ok(bytes) => Ok(bytes),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(err) => Err(LiveClientError::InvalidInput(format!(
            "failed reading `{}`: {err}",
            path.display()
        ))),
    }
}

struct OwnedDiffPair {
    path: String,
    old_text: String,
    new_text: String,
    is_binary: bool,
}

impl OwnedDiffPair {
    fn as_input(&self) -> DiffFileInput<'_> {
        DiffFileInput {
            old_path: &self.path,
            new_path: &self.path,
            old_content: &self.old_text,
            new_content: &self.new_text,
            is_binary: self.is_binary,
        }
    }
    fn from_bytes(path: String, old_bytes: Vec<u8>, new_bytes: Vec<u8>) -> Self {
        match (String::from_utf8(old_bytes), String::from_utf8(new_bytes)) {
            (Ok(old_text), Ok(new_text))
                if !old_text.contains('\0') && !new_text.contains('\0') =>
            {
                Self {
                    path,
                    old_text,
                    new_text,
                    is_binary: false,
                }
            }
            _ => Self {
                path,
                old_text: String::new(),
                new_text: String::new(),
                is_binary: true,
            },
        }
    }
}

struct RawProcessOutput {
    status_code: Option<i32>,
    success: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

async fn execute_raw_bytes(
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

fn cli_io_err(program: &str, err: &std::io::Error) -> LiveClientError {
    LiveClientError::Cli {
        program: program.to_owned(),
        reason: err.to_string(),
    }
}

async fn execute_git_checked(
    repo_dir: &Path,
    args: &[&str],
    base_env: &[(String, String)],
) -> Result<Vec<u8>, LiveClientError> {
    let owned: Vec<String> = args.iter().map(|s| (*s).to_owned()).collect();
    let raw = execute_raw_bytes("git", &owned, base_env, &[], None, Some(repo_dir)).await?;
    if !raw.success {
        return Err(LiveClientError::CliExit {
            program: "git".to_owned(),
            code: raw.status_code,
            stderr: String::from_utf8_lossy(&raw.stderr).trim().to_owned(),
        });
    }
    Ok(raw.stdout)
}

async fn read_git_blob(
    repo_dir: &Path,
    base_ref: &str,
    path: &str,
    base_env: &[(String, String)],
) -> Result<Vec<u8>, LiveClientError> {
    let args = ["show".to_owned(), format!("{base_ref}:{path}")];
    let raw = execute_raw_bytes("git", &args, base_env, &[], None, Some(repo_dir)).await?;
    if raw.success {
        return Ok(raw.stdout);
    }
    let stderr = String::from_utf8_lossy(&raw.stderr);
    if stderr.contains("does not exist in") || stderr.contains("exists on disk, but not in") {
        Ok(Vec::new())
    } else {
        Err(LiveClientError::CliExit {
            program: "git".to_owned(),
            code: raw.status_code,
            stderr: stderr.trim().to_owned(),
        })
    }
}

fn parse_name_status_z(raw: &[u8]) -> Vec<(char, String)> {
    let tokens: Vec<&[u8]> = raw.split(|&b| b == 0).collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 1 < tokens.len() {
        let status_bytes = tokens[i];
        let path_bytes = tokens[i + 1];
        i += 2;
        if status_bytes.is_empty() || path_bytes.is_empty() {
            continue;
        }
        let status = char::from(status_bytes[0]);
        if let Ok(path) = std::str::from_utf8(path_bytes) {
            out.push((status, path.to_owned()));
        }
    }
    out
}
