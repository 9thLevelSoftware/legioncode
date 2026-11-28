#![forbid(unsafe_code)]
//! Active vs stale `.legion-cli/` run lock detection (`engine.lock`, `live-spawn`, `resume.json`).

use std::fs;
use std::path::{Path, PathBuf};

use legion_contracts::workflow_board::{
    WorkflowBoardCliImportLockEntry, WorkflowBoardCliImportLockKind,
    WorkflowBoardCliImportLockStatus, WorkflowBoardRequestError, WorkflowBoardRequestErrorCode,
};
use legion_persistence::is_process_alive;
use serde_json::Value;

use crate::workflow_board_fs::is_symlink_or_reparse_point;

fn local_hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn local_node_platform() -> &'static str {
    match std::env::consts::OS {
        "windows" => "win32",
        "macos" => "darwin",
        other => other,
    }
}

fn is_local_host(hostname: Option<&str>, platform: Option<&str>) -> bool {
    if let Some(p) = platform
        && !p.trim().eq_ignore_ascii_case(local_node_platform())
    {
        return false;
    }
    if let Some(h) = hostname {
        let local = local_hostname();
        if !local.is_empty() && !h.trim().eq_ignore_ascii_case(&local) {
            return false;
        }
    }
    true
}

fn is_regular_non_symlink_file(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .is_ok_and(|meta| !is_symlink_or_reparse_point(&meta) && meta.is_file())
}

fn is_regular_non_symlink_dir(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .is_ok_and(|meta| !is_symlink_or_reparse_point(&meta) && meta.is_dir())
}

fn collect_json_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if !is_regular_non_symlink_dir(dir) {
        return out;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if is_regular_non_symlink_file(&path)
            && path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("json"))
        {
            out.push(path);
        }
    }
    out.sort();
    out
}

pub(crate) fn scan_cli_locks(
    cli_root: &Path,
    cli_rel_prefix: &str,
) -> Result<
    (
        Vec<WorkflowBoardCliImportLockEntry>,
        Vec<WorkflowBoardCliImportLockEntry>,
    ),
    WorkflowBoardRequestError,
> {
    let mut active = Vec::new();
    let mut stale = Vec::new();

    let index_dir = cli_root.join("index");
    if is_regular_non_symlink_dir(&index_dir) {
        for name in ["engine.lock", "engine.lock.steal"] {
            let path = index_dir.join(name);
            if is_regular_non_symlink_file(&path) {
                let rel = format!("{cli_rel_prefix}/index/{name}");
                let entry =
                    inspect_lock_file(&path, &rel, WorkflowBoardCliImportLockKind::EngineLock)?;
                push_lock(&mut active, &mut stale, entry);
            }
        }
    }

    let cache_dir = cli_root.join("cache");
    if is_regular_non_symlink_dir(&cache_dir) {
        let legacy_live = cache_dir.join("live-spawn.json");
        if is_regular_non_symlink_file(&legacy_live) {
            let rel = format!("{cli_rel_prefix}/cache/live-spawn.json");
            let entry = inspect_lock_file(
                &legacy_live,
                &rel,
                WorkflowBoardCliImportLockKind::LiveSpawn,
            )?;
            push_lock(&mut active, &mut stale, entry);
        }

        for folder in ["live-spawn", "live-spawns"] {
            let dir = cache_dir.join(folder);
            for path in collect_json_files(&dir) {
                let file_name = path
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let rel = format!("{cli_rel_prefix}/cache/{folder}/{file_name}");
                let entry =
                    inspect_lock_file(&path, &rel, WorkflowBoardCliImportLockKind::LiveSpawn)?;
                push_lock(&mut active, &mut stale, entry);
            }
        }

        let runs_dir = cache_dir.join("runs");
        if is_regular_non_symlink_dir(&runs_dir)
            && let Ok(entries) = fs::read_dir(&runs_dir)
        {
            let mut run_dirs: Vec<PathBuf> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| is_regular_non_symlink_dir(p))
                .collect();
            run_dirs.sort();
            for run_dir in run_dirs {
                let resume_path = run_dir.join("resume.json");
                if is_regular_non_symlink_file(&resume_path) {
                    let run_name = run_dir
                        .file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let rel = format!("{cli_rel_prefix}/cache/runs/{run_name}/resume.json");
                    let entry = inspect_lock_file(
                        &resume_path,
                        &rel,
                        WorkflowBoardCliImportLockKind::ResumeJson,
                    )?;
                    push_lock(&mut active, &mut stale, entry);
                }
            }
        }
    }

    Ok((active, stale))
}

fn push_lock(
    active: &mut Vec<WorkflowBoardCliImportLockEntry>,
    stale: &mut Vec<WorkflowBoardCliImportLockEntry>,
    entry: WorkflowBoardCliImportLockEntry,
) {
    match entry.status {
        WorkflowBoardCliImportLockStatus::Active => active.push(entry),
        WorkflowBoardCliImportLockStatus::Stale => stale.push(entry),
    }
}

fn inspect_lock_file(
    path: &Path,
    rel_path: &str,
    kind: WorkflowBoardCliImportLockKind,
) -> Result<WorkflowBoardCliImportLockEntry, WorkflowBoardRequestError> {
    let raw = fs::read_to_string(path).map_err(|e| {
        WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::ContractConflict,
            format!("Failed to read active CLI lock `{rel_path}`: {e}"),
        )
    })?;
    let val: Value = serde_json::from_str(raw.trim()).unwrap_or(Value::Null);
    let pid_u64 = val
        .get("pid")
        .and_then(Value::as_u64)
        .filter(|p| *p > 0)
        .or_else(|| {
            val.get("enginePid")
                .and_then(Value::as_u64)
                .filter(|p| *p > 0)
        })
        .or_else(|| {
            val.get("agentPid")
                .and_then(Value::as_u64)
                .filter(|p| *p > 0)
        });
    let agent_pid_u64 = val
        .get("agentPid")
        .and_then(Value::as_u64)
        .filter(|p| *p > 0);
    let run_id = val
        .get("runId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let started_at = val
        .get("startedAt")
        .or_else(|| val.get("acquiredAt"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let hostname = val.get("hostname").and_then(Value::as_str);
    let platform = val.get("platform").and_then(Value::as_str);
    let stage = val.get("stage").and_then(Value::as_str).map(str::trim);
    let uncertain = val
        .get("childTerminationUncertain")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let pid_alive = |pid: u64| -> bool { i64::try_from(pid).is_ok_and(is_process_alive) };
    let local = is_local_host(hostname, platform);

    let is_active = match kind {
        WorkflowBoardCliImportLockKind::EngineLock => {
            if !local || uncertain {
                true
            } else if let Some(pid) = pid_u64 {
                pid_alive(pid)
            } else {
                true
            }
        }
        WorkflowBoardCliImportLockKind::LiveSpawn => {
            if !local || uncertain {
                true
            } else {
                let primary = pid_u64.is_some_and(pid_alive);
                let secondary = agent_pid_u64.is_some_and(pid_alive);
                primary || secondary || (pid_u64.is_none() && agent_pid_u64.is_none())
            }
        }
        WorkflowBoardCliImportLockKind::ResumeJson => {
            if stage == Some("completed") && !uncertain && !pid_u64.is_some_and(pid_alive) {
                false
            } else if !local || uncertain {
                true
            } else {
                pid_u64.is_none_or(pid_alive)
            }
        }
    };

    let status = if is_active {
        WorkflowBoardCliImportLockStatus::Active
    } else {
        WorkflowBoardCliImportLockStatus::Stale
    };
    let message = match status {
        WorkflowBoardCliImportLockStatus::Active => format!(
            "Active CLI run lock at `{rel_path}`{}",
            pid_u64.map_or_else(String::new, |p| format!(" (pid {p})"))
        ),
        WorkflowBoardCliImportLockStatus::Stale => format!(
            "Stale CLI lock at `{rel_path}` (process {} is no longer running)",
            pid_u64.map_or_else(|| "unknown".to_owned(), |p| p.to_string())
        ),
    };

    Ok(WorkflowBoardCliImportLockEntry {
        kind,
        relative_path: rel_path.to_owned(),
        status,
        pid: pid_u64,
        run_id,
        started_at,
        message,
    })
}
