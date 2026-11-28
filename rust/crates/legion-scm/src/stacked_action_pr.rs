#![forbid(unsafe_code)]
//! Pull-request creation/lookup and toast summary builder for `legion_scm::stacked_action`.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::stacked_action::{CommitStepOutcome, PrStepOutcome, PushStepOutcome};
use crate::stacked_action_exec::{
    StackedActionError, StackedActionPhase, resolve_base_branch, resolve_primary_remote, run_gh_ok,
    run_git_ok, run_tool,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToastCtaOutcome {
    None,
    OpenPr { label: String, url: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToastOutcome {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub cta: ToastCtaOutcome,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhPrJson {
    number: u64,
    url: String,
    title: String,
    base_ref_name: Option<String>,
    head_ref_name: Option<String>,
}

pub(crate) async fn step_create_pr(
    cwd: &Path,
    gh_program: &str,
    head_branch: &str,
    commit_subject: Option<&str>,
) -> Result<PrStepOutcome, StackedActionError> {
    let existing_raw = run_gh_ok(
        cwd,
        gh_program,
        &[
            "pr",
            "list",
            "--head",
            head_branch,
            "--state",
            "open",
            "--json",
            "number,url,title,baseRefName,headRefName",
            "--limit",
            "1",
        ],
        None,
    )
    .await?;
    if let Ok(list) = serde_json::from_str::<Vec<GhPrJson>>(&existing_raw)
        && let Some(first) = list.into_iter().next()
    {
        return Ok(PrStepOutcome {
            status: "opened_existing",
            url: Some(first.url),
            number: Some(first.number),
            base_branch: first.base_ref_name,
            head_branch: first.head_ref_name.or_else(|| Some(head_branch.to_owned())),
            title: Some(first.title),
        });
    }

    let remote = resolve_primary_remote(cwd, StackedActionPhase::Pr).await?;
    let base_branch = resolve_base_branch(cwd, &remote).await;
    if head_branch == base_branch {
        return Err(StackedActionError::new(
            Some(StackedActionPhase::Pr),
            "gh pr create",
            format!("Cannot create a pull request from base branch `{base_branch}`"),
        ));
    }
    create_new_github_pr(cwd, gh_program, head_branch, &base_branch, commit_subject).await
}

async fn create_new_github_pr(
    cwd: &Path,
    gh_program: &str,
    head_branch: &str,
    base_branch: &str,
    commit_subject: Option<&str>,
) -> Result<PrStepOutcome, StackedActionError> {
    let title = match commit_subject {
        Some(s) if !s.trim().is_empty() => s.trim().to_owned(),
        _ => run_tool(
            cwd,
            Some(StackedActionPhase::Pr),
            "git",
            &["log", "-1", "--pretty=%s"],
            None,
        )
        .await
        .ok()
        .filter(|(ok, out, _)| *ok && !out.trim().is_empty())
        .map_or_else(
            || head_branch.to_owned(),
            |(_, out, _)| out.trim().to_owned(),
        ),
    };
    let body = run_tool(
        cwd,
        Some(StackedActionPhase::Pr),
        "git",
        &["log", "-1", "--pretty=%b"],
        None,
    )
    .await
    .ok()
    .map_or_default(|(_, out, _)| out.trim().to_owned());

    let created_out = run_gh_ok(
        cwd,
        gh_program,
        &[
            "pr",
            "create",
            "--base",
            base_branch,
            "--head",
            head_branch,
            "--title",
            &title,
            "--body-file",
            "-",
        ],
        Some(&body),
    )
    .await?;
    let url = created_out
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_owned();
    let number = url
        .rsplit('/')
        .next()
        .and_then(|seg| seg.parse::<u64>().ok());
    let view = if url.is_empty() {
        None
    } else {
        run_gh_ok(
            cwd,
            gh_program,
            &[
                "pr",
                "view",
                &url,
                "--json",
                "number,url,title,baseRefName,headRefName",
            ],
            None,
        )
        .await
        .ok()
        .and_then(|s| serde_json::from_str::<GhPrJson>(&s).ok())
    };

    if let Some(v) = view {
        Ok(PrStepOutcome {
            status: "created",
            url: Some(v.url),
            number: Some(v.number),
            base_branch: v.base_ref_name.or_else(|| Some(base_branch.to_owned())),
            head_branch: v.head_ref_name.or_else(|| Some(head_branch.to_owned())),
            title: Some(v.title),
        })
    } else {
        Ok(PrStepOutcome {
            status: "created",
            url: (!url.is_empty()).then_some(url),
            number,
            base_branch: Some(base_branch.to_owned()),
            head_branch: Some(head_branch.to_owned()),
            title: Some(title),
        })
    }
}

pub(crate) fn build_toast(
    commit: &CommitStepOutcome,
    push: &PushStepOutcome,
    pr: &PrStepOutcome,
) -> ToastOutcome {
    if matches!(pr.status, "created" | "opened_existing") {
        let title = match pr.number {
            Some(n) if pr.status == "opened_existing" => format!("Opened pull request #{n}"),
            Some(n) => format!("Created pull request #{n}"),
            None => "Pull request ready".to_owned(),
        };
        let cta = match pr.url {
            Some(ref url) => ToastCtaOutcome::OpenPr {
                label: "Open PR".to_owned(),
                url: url.clone(),
            },
            None => ToastCtaOutcome::None,
        };
        return ToastOutcome {
            title,
            description: pr.title.clone(),
            cta,
        };
    }
    let title = if push.status == "pushed" {
        "Pushed branch to remote"
    } else if push.status == "skipped_up_to_date" {
        "Branch already up to date"
    } else if commit.status == "created" {
        "Created git commit"
    } else {
        "No changes to commit"
    };
    ToastOutcome {
        title: title.to_owned(),
        description: commit.subject.clone(),
        cta: ToastCtaOutcome::None,
    }
}

pub(crate) async fn step_push(
    cwd: &Path,
    branch: &str,
) -> Result<PushStepOutcome, StackedActionError> {
    let upstream_res = run_tool(
        cwd,
        Some(StackedActionPhase::Push),
        "git",
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
        None,
    )
    .await?;
    if upstream_res.0 {
        let full_up = upstream_res.1.trim().to_owned();
        let (ahead_ok, counts, _) = run_tool(
            cwd,
            Some(StackedActionPhase::Push),
            "git",
            &["rev-list", "--left-right", "--count", "HEAD...@{u}"],
            None,
        )
        .await?;
        let ahead = ahead_ok
            && counts
                .split_whitespace()
                .next()
                .and_then(|s| s.parse::<u64>().ok())
                == Some(0);
        if ahead {
            return Ok(PushStepOutcome {
                status: "skipped_up_to_date",
                branch: Some(branch.to_owned()),
                upstream_branch: Some(full_up),
                set_upstream: Some(false),
            });
        }
        let (remote, up_branch) = full_up.split_once('/').unwrap_or(("origin", branch));
        let refspec = format!("HEAD:{up_branch}");
        run_git_ok(
            cwd,
            StackedActionPhase::Push,
            &["push", remote, &refspec],
            None,
        )
        .await?;
        return Ok(PushStepOutcome {
            status: "pushed",
            branch: Some(branch.to_owned()),
            upstream_branch: Some(full_up),
            set_upstream: Some(false),
        });
    }

    let remote = resolve_primary_remote(cwd, StackedActionPhase::Push).await?;
    let refspec = format!("HEAD:{branch}");
    run_git_ok(
        cwd,
        StackedActionPhase::Push,
        &["push", "-u", &remote, &refspec],
        None,
    )
    .await?;
    Ok(PushStepOutcome {
        status: "pushed",
        branch: Some(branch.to_owned()),
        upstream_branch: Some(format!("{remote}/{branch}")),
        set_upstream: Some(true),
    })
}
