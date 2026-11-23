//! `GitHub` workflow check normalization dropping superseded `pull_request` / `pull_request_target`
//! cancelled checks (`#30434`) and `watch_pull_request` immediate pass guidance (`#40866`).

use std::collections::HashMap;

use legion_contracts::pull_requests::PullRequestChecksState;

use crate::core::pr_helpers::{CheckEntry, RawCheckState, sort_checks};

/// Guidance string returned when `watch_pull_request` starts watching a PR whose checks already pass (`#40866`).
pub const CHECKS_ALREADY_PASSED_WATCH_NEXT: &str = "Checks already pass on this head, so no wake will come for them. Act on that now; the watch still wakes you on later changes.";

/// Guidance message returned when `watch_pull_request` sees the PR already merged or closed (`#40866`).
#[must_use]
pub fn already_ended_watch_next(ended_state: &str) -> String {
    format!(
        "This PR is already {ended_state}, so there is nothing to watch. Report that and end your turn."
    )
}

/// Computes the optional `next` instruction hint for `watch_pull_request` results (`#40866`).
#[must_use]
pub fn resolve_watch_pull_request_next(
    ended_state: Option<&str>,
    checks_state: PullRequestChecksState,
) -> Option<String> {
    if let Some(ended) = ended_state
        && !ended.trim().is_empty()
    {
        return Some(already_ended_watch_next(ended));
    }
    if checks_state == PullRequestChecksState::Passing {
        return Some(CHECKS_ALREADY_PASSED_WATCH_NEXT.to_owned());
    }
    None
}

/// Metadata identifying the Actions workflow run that produced a `GitHub` check run (`#30434`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubWorkflowRunRef {
    /// Workflow trigger event (`"pull_request"`, `"pull_request_target"`, `"push"`, etc.).
    pub event: String,
    /// Monotonic run number within the workflow.
    pub run_number: u64,
    /// GraphQL node ID of the Actions workflow file.
    pub workflow_id: String,
}

/// Raw check-run or commit-status entry extracted from `GitHub`'s `statusCheckRollup` (`#30434`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawGitHubCheckContext {
    /// `CheckRun.name`.
    pub name: Option<String>,
    /// `StatusContext.context`.
    pub context: Option<String>,
    /// `CheckRun.status` (e.g. `"COMPLETED"`, `"IN_PROGRESS"`).
    pub status: Option<String>,
    /// `CheckRun.conclusion` or `StatusContext.state`.
    pub conclusion: Option<String>,
    /// Whether branch protection requires this check.
    pub is_required: bool,
    /// Best completion/start timestamp string.
    pub updated_at: Option<String>,
    /// Details URL.
    pub details_url: Option<String>,
    /// Optional owning workflow run metadata (`None` for commit statuses / app-owned runs).
    pub workflow_run: Option<GitHubWorkflowRunRef>,
}

/// Normalizes `GitHub` check contexts while excluding cancelled checks from older superseded
/// `pull_request` / `pull_request_target` workflow runs (`#30434`).
///
/// Required cancelled checks are only dropped when a newer run of the **same check name** exists
/// under the same `(workflow_id, event)` lineage.
#[must_use]
pub fn normalize_github_check_contexts(raw: &[RawGitHubCheckContext]) -> Vec<CheckEntry> {
    let mut newest_run_number: HashMap<String, u64> = HashMap::new();
    for check in raw {
        let Some(run) = &check.workflow_run else {
            continue;
        };
        if run.event != "pull_request" && run.event != "pull_request_target" {
            continue;
        }
        let lineage = format!("{} {}", run.workflow_id, run.event);
        note_max_run(&mut newest_run_number, lineage.clone(), run.run_number);
        let trimmed_name = check.name.as_deref().map_or("", str::trim);
        note_max_run(
            &mut newest_run_number,
            format!("{lineage} {trimmed_name}"),
            run.run_number,
        );
    }

    let mut out = Vec::new();
    for check in raw {
        let resolved_name = check
            .name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .or_else(|| {
                check
                    .context
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
            });
        let Some(name) = resolved_name else {
            continue;
        };
        let state = RawCheckState::from_forge_str(
            check
                .conclusion
                .as_deref()
                .or(check.status.as_deref())
                .unwrap_or("pending"),
        );
        if state == RawCheckState::Cancelled
            && let Some(run) = &check.workflow_run
        {
            let lineage = format!("{} {}", run.workflow_id, run.event);
            let replaced_by = if check.is_required {
                format!("{lineage} {name}")
            } else {
                lineage
            };
            if let Some(&newest) = newest_run_number.get(&replaced_by)
                && run.run_number < newest
            {
                continue;
            }
        }
        out.push(CheckEntry {
            name: name.to_owned(),
            context: check.context.clone(),
            state,
            required: check.is_required,
            updated_at: check.updated_at.clone(),
            details_url: check.details_url.clone(),
        });
    }
    sort_checks(&mut out);
    out
}

fn note_max_run(map: &mut HashMap<String, u64>, key: String, run_number: u64) {
    map.entry(key)
        .and_modify(|cur| {
            if run_number > *cur {
                *cur = run_number;
            }
        })
        .or_insert(run_number);
}
