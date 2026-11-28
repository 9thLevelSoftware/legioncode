//! Git, worktree, and disposable `.legion-cli` fixture helpers for opaque-box E2E tests.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use std::time::{SystemTime, UNIX_EPOCH};

use super::TestResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliLockKind {
    EngineLock,
    LiveSpawn,
    ResumeJson,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CopyLegionCliOpts {
    pub include_map: bool,
    pub include_cache: bool,
}

fn run_git(dir: &Path, args: &[&str]) -> TestResult<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "")
        .output()?;
    if !out.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        )
        .into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

pub fn init_git_repo(dir: &Path) -> TestResult<()> {
    fs::create_dir_all(dir)?;
    if dir.join(".git").exists() {
        return Ok(());
    }
    run_git(dir, &["init", "-b", "main"])?;
    run_git(dir, &["config", "user.name", "Workflow E2E"])?;
    run_git(dir, &["config", "user.email", "e2e@legion.codes"])?;
    run_git(dir, &["config", "core.autocrlf", "false"])?;
    fs::write(dir.join("README.md"), "# Workflow Board E2E\n")?;
    run_git(dir, &["add", "README.md"])?;
    run_git(dir, &["commit", "-m", "init"])?;
    Ok(())
}

pub fn create_worktree(repo_dir: &Path, wt_dir: &Path, branch: &str) -> TestResult<()> {
    if let Some(parent) = wt_dir.parent() {
        fs::create_dir_all(parent)?;
    }
    let wt_str = wt_dir.to_string_lossy().into_owned();
    run_git(
        repo_dir,
        &["worktree", "add", "-b", branch, &wt_str, "HEAD"],
    )?;
    Ok(())
}

pub fn commit_file(
    repo_dir: &Path,
    rel_path: &str,
    content: &str,
    message: &str,
) -> TestResult<String> {
    let abs = repo_dir.join(rel_path);
    if let Some(parent) = abs.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&abs, content)?;
    run_git(repo_dir, &["add", rel_path])?;
    run_git(repo_dir, &["commit", "-m", message])?;
    run_git(repo_dir, &["rev-parse", "HEAD"])
}

fn copy_dir_recursive(src: &Path, dst: &Path, skip_names: &[&str]) -> TestResult<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if skip_names.contains(&name_str.as_ref()) {
            continue;
        }
        let target = dst.join(&name);
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&entry.path(), &target, &[])?;
        } else {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

pub fn copy_disposable_legion_cli(
    dest_workspace: &Path,
    opts: CopyLegionCliOpts,
) -> TestResult<PathBuf> {
    let source_cli = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
        .join(".legion-cli");
    let dest_cli = dest_workspace.join(".legion-cli");
    let mut skips = Vec::new();
    if !opts.include_map {
        skips.push("map");
    }
    if !opts.include_cache {
        skips.push("cache");
    }
    copy_dir_recursive(&source_cli, &dest_cli, &skips)?;
    Ok(dest_cli)
}

fn now_iso_utc() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0_i64, |d| i64::try_from(d.as_secs()).unwrap_or(0));
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let hour = rem / 3_600;
    let minute = (rem % 3_600) / 60;
    let second = rem % 60;

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn node_platform_name() -> &'static str {
    match std::env::consts::OS {
        "windows" => "win32",
        "macos" => "darwin",
        other => other,
    }
}

fn local_hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "localhost".to_owned())
}

pub fn seed_active_cli_lock(dest_workspace: &Path, kind: CliLockKind) -> TestResult<PathBuf> {
    let cli_dir = dest_workspace.join(".legion-cli");
    let pid = std::process::id();
    let run_id = "synthetic-active-run";
    let skill_id = "workflow-execute";
    let task_id = "TSK-0001";
    let now_iso = now_iso_utc();
    let host = local_hostname();
    let platform = node_platform_name();

    let (lock_path, payload) = match kind {
        CliLockKind::EngineLock => (
            cli_dir.join("index").join("engine.lock"),
            serde_json::json!({
                "pid": pid,
                "acquiredAt": now_iso,
                "hostname": host,
                "platform": platform,
            }),
        ),
        CliLockKind::LiveSpawn => (
            cli_dir
                .join("cache")
                .join("live-spawn")
                .join(format!("{run_id}.json")),
            serde_json::json!({
                "schemaVersion": "legion-cli-live-run/v1",
                "runId": run_id,
                "skillId": skill_id,
                "taskId": task_id,
                "enginePid": pid,
                "agentPid": null,
                "startedAt": now_iso,
                "hostname": host,
                "platform": platform,
            }),
        ),
        CliLockKind::ResumeJson => (
            cli_dir
                .join("cache")
                .join("runs")
                .join(run_id)
                .join("resume.json"),
            serde_json::json!({
                "schemaVersion": "legion-cli-resume/v1",
                "runId": run_id,
                "skillId": skill_id,
                "taskId": task_id,
                "pid": pid,
                "enginePid": null,
                "startedAt": now_iso,
            }),
        ),
    };
    if let Some(parent) = lock_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&lock_path, serde_json::to_string_pretty(&payload)?)?;
    Ok(lock_path)
}
