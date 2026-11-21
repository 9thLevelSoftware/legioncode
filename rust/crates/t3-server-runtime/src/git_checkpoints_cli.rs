//! Low-level `git` CLI helpers with `GIT_TERMINAL_PROMPT=0` isolation.

use std::path::Path;
use std::process::Command;

use t3_contracts::orchestration::CheckpointFileSummary;
use t3_scm::checkpoints::rollback::GitRestorePlan;
use t3_scm::checkpoints::validate_git_ref_component;
use tempfile::TempDir;

use crate::error::ServerRuntimeError;

const CHECKPOINT_REF_PREFIX: &str = "refs/t3/orchestration-v2/checkpoints";

/// Build the canonical hidden git ref for a checkpoint ordinal.
pub fn build_checkpoint_ref(scope: &str, ordinal: u64) -> Result<String, ServerRuntimeError> {
    validate_git_ref_component(scope)?;
    Ok(format!("{CHECKPOINT_REF_PREFIX}/{scope}/ordinal/{ordinal}"))
}

/// Run a `git` command in `workspace_root` with non-interactive terminal settings.
pub(crate) fn run_git(
    workspace_root: &Path,
    args: &[&str],
    extra_env: &[(&str, &str)],
) -> Result<Vec<u8>, ServerRuntimeError> {
    let mut cmd = Command::new("git");
    cmd.current_dir(workspace_root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "")
        .args(args);
    for (key, value) in extra_env {
        cmd.env(key, value);
    }
    let output = cmd.output()?;
    if output.status.success() {
        return Ok(output.stdout);
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let detail = if stderr.is_empty() { stdout } else { stderr };
    Err(ServerRuntimeError::Git {
        command: format!("git {}", args.join(" ")),
        message: detail,
    })
}

/// Check whether `workspace_root` is inside a valid git work tree.
pub fn is_git_workspace(workspace_root: &Path) -> bool {
    run_git(workspace_root, &["rev-parse", "--is-inside-work-tree"], &[])
        .is_ok_and(|bytes| String::from_utf8_lossy(&bytes).trim() == "true")
}

/// Resolve a git ref or revision expression to a commit SHA, or `None` if absent.
pub fn resolve_commit_oid(workspace_root: &Path, rev: &str) -> Option<String> {
    let target = format!("{rev}^{{commit}}");
    let out = run_git(workspace_root, &["rev-parse", "--verify", &target], &[]).ok()?;
    let oid = String::from_utf8_lossy(&out).trim().to_owned();
    if oid.is_empty() { None } else { Some(oid) }
}

/// List currently tracked workspace paths using NUL-delimited `git ls-files -z`.
pub fn list_tracked_paths(workspace_root: &Path) -> Result<Vec<String>, ServerRuntimeError> {
    let bytes = run_git(workspace_root, &["ls-files", "-z", "--", "."], &[])?;
    Ok(bytes
        .split(|&b| b == 0)
        .filter(|entry| !entry.is_empty())
        .map(|entry| String::from_utf8_lossy(entry).into_owned())
        .collect())
}

/// Capture all workspace files (staged, unstaged, untracked) into a snapshot commit under `ref_name`.
pub fn capture_snapshot_commit(
    workspace_root: &Path,
    ref_name: &str,
    message: &str,
) -> Result<String, ServerRuntimeError> {
    if !is_git_workspace(workspace_root) {
        return Err(ServerRuntimeError::Conflict(format!(
            "`{}` is not a git work tree",
            workspace_root.display()
        )));
    }
    let temp_dir = TempDir::new()?;
    let index_path = temp_dir.path().join("index").to_string_lossy().into_owned();
    let head_oid = resolve_commit_oid(workspace_root, "HEAD");
    if let Some(ref head) = head_oid {
        run_git(
            workspace_root,
            &["read-tree", "--reset", head],
            &[("GIT_INDEX_FILE", &index_path)],
        )?;
    }
    run_git(
        workspace_root,
        &["add", "-A", "--", "."],
        &[("GIT_INDEX_FILE", &index_path)],
    )?;
    let tree_out = run_git(
        workspace_root,
        &["write-tree"],
        &[("GIT_INDEX_FILE", &index_path)],
    )?;
    let tree_oid = String::from_utf8_lossy(&tree_out).trim().to_owned();
    let commit_env = [
        ("GIT_AUTHOR_NAME", "T3 Code"),
        ("GIT_AUTHOR_EMAIL", "t3code@local"),
        ("GIT_COMMITTER_NAME", "T3 Code"),
        ("GIT_COMMITTER_EMAIL", "t3code@local"),
    ];
    let commit_out = if let Some(ref head) = head_oid {
        run_git(
            workspace_root,
            &["commit-tree", &tree_oid, "-p", head, "-m", message],
            &commit_env,
        )?
    } else {
        run_git(
            workspace_root,
            &["commit-tree", &tree_oid, "-m", message],
            &commit_env,
        )?
    };
    let commit_oid = String::from_utf8_lossy(&commit_out).trim().to_owned();
    run_git(workspace_root, &["update-ref", ref_name, &commit_oid], &[])?;
    Ok(commit_oid)
}

/// Compute per-file numstat summaries and full unified diff text between `base_rev` and `target_rev`.
pub fn compute_checkpoint_diff(
    workspace_root: &Path,
    base_rev: Option<&str>,
    target_rev: &str,
) -> Result<(Vec<CheckpointFileSummary>, String), ServerRuntimeError> {
    let effective_base = base_rev.or_else(|| {
        resolve_commit_oid(workspace_root, "HEAD")
            .is_some()
            .then_some("HEAD")
    });
    let numstat_bytes = if let Some(base) = effective_base {
        run_git(
            workspace_root,
            &["diff", "--numstat", "-M", base, target_rev],
            &[],
        )?
    } else {
        run_git(
            workspace_root,
            &[
                "diff-tree",
                "--no-commit-id",
                "--numstat",
                "-r",
                "--root",
                target_rev,
            ],
            &[],
        )?
    };
    let diff_bytes = if let Some(base) = effective_base {
        run_git(workspace_root, &["diff", base, target_rev], &[])?
    } else {
        run_git(workspace_root, &["show", "--format=", target_rev], &[])?
    };
    let files = parse_numstat(&String::from_utf8_lossy(&numstat_bytes));
    let diff_text = String::from_utf8_lossy(&diff_bytes).into_owned();
    Ok((files, diff_text))
}

fn parse_numstat(raw: &str) -> Vec<CheckpointFileSummary> {
    let mut out = Vec::new();
    for line in raw.lines() {
        let mut parts = line.splitn(3, '\t');
        let Some(add_raw) = parts.next() else {
            continue;
        };
        let Some(del_raw) = parts.next() else {
            continue;
        };
        let Some(path) = parts.next() else {
            continue;
        };
        if path.trim().is_empty() {
            continue;
        }
        let additions = add_raw.trim().parse::<u64>().unwrap_or(0);
        let deletions = del_raw.trim().parse::<u64>().unwrap_or(0);
        out.push(CheckpointFileSummary {
            path: path.trim().to_owned(),
            kind: "modified".to_owned(),
            additions,
            deletions,
        });
    }
    out
}

/// Execute git restore + optional `git clean -fd` according to a verified [`GitRestorePlan`].
pub fn execute_restore_to_ref(
    workspace_root: &Path,
    target_ref: &str,
    plan: &GitRestorePlan,
) -> Result<String, ServerRuntimeError> {
    let commit_oid = resolve_commit_oid(workspace_root, target_ref).ok_or_else(|| {
        ServerRuntimeError::NotFound(format!("checkpoint git ref `{target_ref}` not found"))
    })?;
    let _ = run_git(
        workspace_root,
        &[
            "restore",
            "--source",
            &commit_oid,
            "--worktree",
            "--staged",
            "--",
            ".",
        ],
        &[],
    )?;
    if let Some(clean_args) = &plan.clean_untracked_args {
        let arg_refs: Vec<&str> = clean_args.iter().map(String::as_str).collect();
        let _ = run_git(workspace_root, &arg_refs, &[])?;
    }
    if resolve_commit_oid(workspace_root, "HEAD").is_some() {
        let _ = run_git(workspace_root, &["reset", "--quiet", "--", "."], &[])?;
    }
    Ok(commit_oid)
}
