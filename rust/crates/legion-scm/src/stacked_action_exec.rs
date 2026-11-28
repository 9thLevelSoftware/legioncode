#![forbid(unsafe_code)]
//! Child-process execution and git/gh discovery helpers for stacked actions.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::Serialize;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::live_git_cli::collect_safe_base_env;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StackedActionPhase {
    Branch,
    Commit,
    Push,
    Pr,
}

impl StackedActionPhase {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Branch => "branch",
            Self::Commit => "commit",
            Self::Push => "push",
            Self::Pr => "pr",
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Branch => "Creating feature branch",
            Self::Commit => "Creating git commit",
            Self::Push => "Pushing branch to remote",
            Self::Pr => "Creating pull request",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackedActionError {
    pub phase: Option<StackedActionPhase>,
    pub operation: String,
    pub detail: String,
}

impl StackedActionError {
    #[must_use]
    pub fn new(
        phase: Option<StackedActionPhase>,
        operation: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            phase,
            operation: operation.into(),
            detail: detail.into(),
        }
    }
}

pub(crate) fn resolve_program(prog: &str, base_env: &[(String, String)]) -> PathBuf {
    let p = Path::new(prog);
    if p.components().count() > 1 || p.is_absolute() {
        return p.to_path_buf();
    }
    let Some((_, path_val)) = base_env
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("PATH"))
    else {
        return p.to_path_buf();
    };
    #[cfg(windows)]
    let exts: Vec<String> = {
        let configured: Vec<_> = base_env
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("PATHEXT"))
            .into_iter()
            .flat_map(|(_, value)| value.split(';'))
            .map(str::trim)
            .filter(|ext| !ext.is_empty())
            .map(str::to_ascii_lowercase)
            .collect();
        if configured.is_empty() {
            [".exe", ".cmd", ".bat"].map(str::to_owned).into()
        } else {
            configured
        }
    };
    for dir in std::env::split_paths(path_val) {
        #[cfg(windows)]
        if p.extension().is_none() {
            for ext in &exts {
                let with_ext = dir.join(format!("{prog}{ext}"));
                if with_ext.is_file() {
                    return with_ext;
                }
            }
        }
        let candidate = dir.join(prog);
        if candidate.is_file() {
            return candidate;
        }
    }
    p.to_path_buf()
}

pub(crate) async fn run_tool(
    cwd: &Path,
    phase: Option<StackedActionPhase>,
    prog: &str,
    args: &[&str],
    stdin_data: Option<&str>,
) -> Result<(bool, String, String), StackedActionError> {
    let base_env = collect_safe_base_env();
    let bin = resolve_program(prog, &base_env);
    let op = format!("{prog} {}", args.join(" "));
    let mut cmd = Command::new(&bin);
    cmd.current_dir(cwd)
        .env_clear()
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in &base_env {
        cmd.env(k, v);
    }
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    cmd.env("GIT_ASKPASS", "");
    if stdin_data.is_some() {
        cmd.stdin(Stdio::piped());
    } else {
        cmd.stdin(Stdio::null());
    }

    let mut child = cmd.spawn().map_err(|e| {
        StackedActionError::new(phase, &op, format!("failed to spawn `{prog}`: {e}"))
    })?;
    if let Some(input) = stdin_data
        && let Some(mut stdin) = child.stdin.take()
    {
        stdin.write_all(input.as_bytes()).await.map_err(|e| {
            StackedActionError::new(phase, &op, format!("failed writing stdin: {e}"))
        })?;
        drop(stdin);
    }
    let out = child
        .wait_with_output()
        .await
        .map_err(|e| StackedActionError::new(phase, &op, format!("process wait failed: {e}")))?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    Ok((out.status.success(), stdout, stderr))
}

pub(crate) async fn run_git_ok(
    cwd: &Path,
    phase: StackedActionPhase,
    args: &[&str],
    stdin_data: Option<&str>,
) -> Result<String, StackedActionError> {
    let op = format!("git {}", args.join(" "));
    let (ok, stdout, stderr) = run_tool(cwd, Some(phase), "git", args, stdin_data).await?;
    if ok {
        Ok(stdout)
    } else {
        let msg = if stderr.trim().is_empty() {
            stdout.trim().to_owned()
        } else {
            stderr.trim().to_owned()
        };
        Err(StackedActionError::new(Some(phase), op, msg))
    }
}

pub(crate) async fn run_gh_ok(
    cwd: &Path,
    gh_program: &str,
    args: &[&str],
    stdin_data: Option<&str>,
) -> Result<String, StackedActionError> {
    let op = format!("gh {}", args.join(" "));
    let (ok, stdout, stderr) = run_tool(
        cwd,
        Some(StackedActionPhase::Pr),
        gh_program,
        args,
        stdin_data,
    )
    .await?;
    if ok {
        Ok(stdout)
    } else {
        let msg = if stderr.trim().is_empty() {
            stdout.trim().to_owned()
        } else {
            stderr.trim().to_owned()
        };
        Err(StackedActionError::new(
            Some(StackedActionPhase::Pr),
            op,
            msg,
        ))
    }
}

pub(crate) async fn resolve_primary_remote(
    cwd: &Path,
    phase: StackedActionPhase,
) -> Result<String, StackedActionError> {
    let raw = run_git_ok(cwd, phase, &["remote"], None).await?;
    let remotes: Vec<&str> = raw
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if remotes.contains(&"origin") {
        return Ok("origin".to_owned());
    }
    remotes.first().map(|s| (*s).to_owned()).ok_or_else(|| {
        StackedActionError::new(Some(phase), "git remote", "No git remote configured")
    })
}

pub(crate) async fn resolve_base_branch(cwd: &Path, remote: &str) -> String {
    let head_ref = format!("refs/remotes/{remote}/HEAD");
    if let Ok((true, out, _)) = run_tool(
        cwd,
        Some(StackedActionPhase::Pr),
        "git",
        &["symbolic-ref", "--quiet", "--short", &head_ref],
        None,
    )
    .await
    {
        let trimmed = out.trim();
        let stripped = trimmed
            .strip_prefix(&format!("{remote}/"))
            .unwrap_or(trimmed);
        if !stripped.is_empty() {
            return stripped.to_owned();
        }
    }
    for cand in ["main", "master"] {
        let ref_path = format!("refs/remotes/{remote}/{cand}");
        if let Ok((true, _, _)) = run_tool(
            cwd,
            Some(StackedActionPhase::Pr),
            "git",
            &["show-ref", "--verify", "--quiet", &ref_path],
            None,
        )
        .await
        {
            return cand.to_owned();
        }
    }
    "main".to_owned()
}

#[cfg(test)]
mod tests {
    use super::resolve_program;

    #[cfg(windows)]
    #[test]
    fn resolve_program_prefers_pathext_over_extensionless_on_windows()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let sh_path = tmp.path().join("gh");
        let cmd_path = tmp.path().join("gh.cmd");
        std::fs::write(&sh_path, "#!/bin/sh\necho sh\n")?;
        std::fs::write(&cmd_path, "@echo off\r\necho cmd\r\n")?;

        let env = vec![
            ("PATH".to_owned(), tmp.path().to_string_lossy().into_owned()),
            ("PATHEXT".to_owned(), ".COM;.EXE;.BAT;.CMD".to_owned()),
        ];
        assert_eq!(resolve_program("gh", &env), cmd_path);
        assert_eq!(resolve_program("gh.cmd", &env), cmd_path);
        for empty_extensions in ["", "; ;\t;"] {
            let env = vec![
                ("PATH".to_owned(), tmp.path().to_string_lossy().into_owned()),
                ("PATHEXT".to_owned(), empty_extensions.to_owned()),
            ];
            assert_eq!(resolve_program("gh", &env), cmd_path);
        }
        Ok(())
    }
}
