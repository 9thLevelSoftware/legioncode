#![forbid(unsafe_code)]
//! Real `git.runStackedAction` execution: `branch`, `commit`, `push`, and `create_pr`.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub use crate::stacked_action_exec::{StackedActionError, StackedActionPhase};
use crate::stacked_action_exec::{run_git_ok, run_tool};
pub use crate::stacked_action_pr::{ToastCtaOutcome, ToastOutcome};
use crate::stacked_action_pr::{build_toast, step_create_pr, step_push};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StackedActionKind {
    Commit,
    Push,
    CreatePr,
    CommitPush,
    CommitPushPr,
}

impl StackedActionKind {
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "commit" => Some(Self::Commit),
            "push" => Some(Self::Push),
            "create_pr" => Some(Self::CreatePr),
            "commit_push" => Some(Self::CommitPush),
            "commit_push_pr" => Some(Self::CommitPushPr),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Commit => "commit",
            Self::Push => "push",
            Self::CreatePr => "create_pr",
            Self::CommitPush => "commit_push",
            Self::CommitPushPr => "commit_push_pr",
        }
    }

    #[must_use]
    pub const fn includes_commit(self) -> bool {
        matches!(self, Self::Commit | Self::CommitPush | Self::CommitPushPr)
    }

    #[must_use]
    pub const fn includes_push(self) -> bool {
        matches!(self, Self::Push | Self::CommitPush | Self::CommitPushPr)
    }

    #[must_use]
    pub const fn includes_pr(self) -> bool {
        matches!(self, Self::CreatePr | Self::CommitPushPr)
    }
}

#[derive(Debug, Clone)]
pub struct StackedActionRequest {
    pub cwd: PathBuf,
    pub action: StackedActionKind,
    pub commit_message: Option<String>,
    pub feature_branch: bool,
    pub file_paths: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchStepOutcome {
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitStepOutcome {
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit_sha: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PushStepOutcome {
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub set_upstream: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrStepOutcome {
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head_branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StackedActionOutcome {
    pub action: &'static str,
    pub branch: BranchStepOutcome,
    pub commit: CommitStepOutcome,
    pub push: PushStepOutcome,
    pub pr: PrStepOutcome,
    pub toast: ToastOutcome,
}

#[must_use]
pub fn phases_for_action(
    action: StackedActionKind,
    feature_branch: bool,
) -> Vec<StackedActionPhase> {
    let mut phases = Vec::new();
    if feature_branch {
        phases.push(StackedActionPhase::Branch);
    }
    if action.includes_commit() {
        phases.push(StackedActionPhase::Commit);
    }
    if action.includes_push() {
        phases.push(StackedActionPhase::Push);
    }
    if action.includes_pr() {
        phases.push(StackedActionPhase::Pr);
    }
    phases
}

pub async fn execute_stacked_action<F>(
    req: &StackedActionRequest,
    on_phase: F,
) -> Result<StackedActionOutcome, StackedActionError>
where
    F: FnMut(StackedActionPhase, &str),
{
    execute_stacked_action_with_gh(req, "gh", on_phase).await
}

pub async fn execute_stacked_action_with_gh<F>(
    req: &StackedActionRequest,
    gh_program: &str,
    mut on_phase: F,
) -> Result<StackedActionOutcome, StackedActionError>
where
    F: FnMut(StackedActionPhase, &str),
{
    let cwd = req.cwd.as_path();
    let (branch_step, active_branch) = step_branch(cwd, req.feature_branch, &mut on_phase).await?;

    let commit_step = if req.action.includes_commit() {
        on_phase(
            StackedActionPhase::Commit,
            StackedActionPhase::Commit.label(),
        );
        step_commit(
            cwd,
            req.commit_message.as_deref(),
            req.file_paths.as_deref(),
        )
        .await?
    } else {
        CommitStepOutcome {
            status: "skipped_not_requested",
            commit_sha: None,
            subject: None,
        }
    };

    let push_step = if req.action.includes_push() {
        on_phase(StackedActionPhase::Push, StackedActionPhase::Push.label());
        step_push(cwd, &active_branch).await?
    } else {
        PushStepOutcome {
            status: "skipped_not_requested",
            branch: None,
            upstream_branch: None,
            set_upstream: None,
        }
    };

    let pr_step = if req.action.includes_pr() {
        on_phase(StackedActionPhase::Pr, StackedActionPhase::Pr.label());
        if !req.action.includes_push() {
            let _ = step_push(cwd, &active_branch).await?;
        }
        step_create_pr(
            cwd,
            gh_program,
            &active_branch,
            commit_step.subject.as_deref(),
        )
        .await?
    } else {
        PrStepOutcome {
            status: "skipped_not_requested",
            url: None,
            number: None,
            base_branch: None,
            head_branch: None,
            title: None,
        }
    };

    let toast = build_toast(&commit_step, &push_step, &pr_step);
    Ok(StackedActionOutcome {
        action: req.action.as_str(),
        branch: branch_step,
        commit: commit_step,
        push: push_step,
        pr: pr_step,
        toast,
    })
}

async fn step_branch<F>(
    cwd: &std::path::Path,
    feature_branch: bool,
    on_phase: &mut F,
) -> Result<(BranchStepOutcome, String), StackedActionError>
where
    F: FnMut(StackedActionPhase, &str),
{
    if feature_branch {
        on_phase(
            StackedActionPhase::Branch,
            StackedActionPhase::Branch.label(),
        );
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let name = format!("feat/{:08x}", (nanos & 0xffff_ffff) as u32);
        run_git_ok(
            cwd,
            StackedActionPhase::Branch,
            &["checkout", "-b", &name],
            None,
        )
        .await?;
        return Ok((
            BranchStepOutcome {
                status: "created",
                name: Some(name.clone()),
            },
            name,
        ));
    }
    let cur = run_git_ok(
        cwd,
        StackedActionPhase::Branch,
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
        None,
    )
    .await
    .map_or_default(|s| s.trim().to_owned());
    if cur.is_empty() || cur == "HEAD" {
        return Err(StackedActionError::new(
            Some(StackedActionPhase::Branch),
            "git symbolic-ref HEAD",
            "Detached HEAD; switch to a branch or enable featureBranch",
        ));
    }
    Ok((
        BranchStepOutcome {
            status: "skipped_not_requested",
            name: Some(cur.clone()),
        },
        cur,
    ))
}

async fn step_commit(
    cwd: &std::path::Path,
    commit_message: Option<&str>,
    file_paths: Option<&[String]>,
) -> Result<CommitStepOutcome, StackedActionError> {
    let has_paths = file_paths.is_some_and(|p| !p.is_empty());
    let status_out = if let Some(paths) = file_paths.filter(|p| !p.is_empty()) {
        let mut args = vec!["status", "--porcelain", "--"];
        args.extend(paths.iter().map(String::as_str));
        run_git_ok(cwd, StackedActionPhase::Commit, &args, None).await?
    } else {
        run_git_ok(
            cwd,
            StackedActionPhase::Commit,
            &["status", "--porcelain"],
            None,
        )
        .await?
    };
    if status_out.trim().is_empty() {
        return Ok(CommitStepOutcome {
            status: "skipped_no_changes",
            commit_sha: None,
            subject: None,
        });
    }

    if let Some(paths) = file_paths.filter(|p| !p.is_empty()) {
        let mut args = vec!["add", "--"];
        args.extend(paths.iter().map(String::as_str));
        run_git_ok(cwd, StackedActionPhase::Commit, &args, None).await?;
    } else if !has_paths {
        run_git_ok(cwd, StackedActionPhase::Commit, &["add", "-A"], None).await?;
    }
    let (diff_empty, _, _) = if let Some(paths) = file_paths.filter(|p| !p.is_empty()) {
        let mut diff_args = vec!["diff", "--cached", "--quiet", "--"];
        diff_args.extend(paths.iter().map(String::as_str));
        run_tool(
            cwd,
            Some(StackedActionPhase::Commit),
            "git",
            &diff_args,
            None,
        )
        .await?
    } else {
        run_tool(
            cwd,
            Some(StackedActionPhase::Commit),
            "git",
            &["diff", "--cached", "--quiet"],
            None,
        )
        .await?
    };
    if diff_empty {
        return Ok(CommitStepOutcome {
            status: "skipped_no_changes",
            commit_sha: None,
            subject: None,
        });
    }

    let msg = commit_message
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("chore: save workspace changes");
    let subject = msg.lines().next().unwrap_or(msg).trim().to_owned();
    if let Some(paths) = file_paths.filter(|p| !p.is_empty()) {
        let mut commit_args = vec!["commit", "--only", "-F", "-", "--"];
        commit_args.extend(paths.iter().map(String::as_str));
        run_git_ok(cwd, StackedActionPhase::Commit, &commit_args, Some(msg)).await?;
    } else {
        run_git_ok(
            cwd,
            StackedActionPhase::Commit,
            &["commit", "-F", "-"],
            Some(msg),
        )
        .await?;
    }
    let sha = run_git_ok(
        cwd,
        StackedActionPhase::Commit,
        &["rev-parse", "HEAD"],
        None,
    )
    .await?
    .trim()
    .to_owned();
    Ok(CommitStepOutcome {
        status: "created",
        commit_sha: Some(sha),
        subject: Some(subject),
    })
}
