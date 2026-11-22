//! `GitLab` `glab` CLI builder (`GITLAB_HOST`, Snap-safe `stdin` descriptions) and MR normalizer.

use legion_contracts::pull_requests::{PullRequestChecksState, PullRequestMergeability};
use serde_json::Value;

use crate::forges::{
    CliCommandSpec, ForgeError, NormalizedPullRequest, json_str, json_u64, parse_standard_pr_state,
    validate_hostname,
};

/// Default `GitLab` cloud hostname.
pub const GITLAB_DEFAULT_HOST: &str = "gitlab.com";

/// Classified `glab` CLI error categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitLabErrorKind {
    /// Expired OAuth grant or missing `glab auth login` session.
    AuthExpired,
    /// Upstream HTTP `429` or `GitLab` rate limit message.
    RateLimited,
    /// Resource not found.
    NotFound,
    /// Unclassified command failure.
    Unknown,
}

/// Typed `glab` CLI builder ensuring `GITLAB_HOST` and Snap-safe stdin flags are set consistently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitLabCliBuilder {
    host: String,
    repo_path: Option<String>,
}

impl GitLabCliBuilder {
    /// Creates a [`GitLabCliBuilder`] bound to `host` (defaults to `gitlab.com` when `None`).
    ///
    /// # Errors
    ///
    /// Returns [`ForgeError::InvalidHost`] if a provided hostname is malformed.
    pub fn new(host: Option<&str>, repo_path: Option<&str>) -> Result<Self, ForgeError> {
        let resolved_host = match host {
            Some(h) if !h.trim().is_empty() => validate_hostname(h)?,
            _ => GITLAB_DEFAULT_HOST.to_owned(),
        };
        Ok(Self {
            host: resolved_host,
            repo_path: repo_path
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned),
        })
    }

    /// Builds `glab mr list --output json` scoped with `GITLAB_HOST`.
    #[must_use]
    pub fn mr_list(&self, source_branch: Option<&str>) -> CliCommandSpec {
        let mut args = vec![
            "mr".to_owned(),
            "list".to_owned(),
            "--output".to_owned(),
            "json".to_owned(),
        ];
        if let Some(branch) = source_branch {
            args.push("--source-branch".to_owned());
            args.push(branch.to_owned());
        }
        self.finish(args, None)
    }

    /// Builds `glab mr view <iid> --output json` scoped with `GITLAB_HOST`.
    #[must_use]
    pub fn mr_view(&self, iid: u64) -> CliCommandSpec {
        let args = vec![
            "mr".to_owned(),
            "view".to_owned(),
            iid.to_string(),
            "--output".to_owned(),
            "json".to_owned(),
        ];
        self.finish(args, None)
    }

    /// Builds `glab mr create` passing `description` via `--description -` on `stdin`
    /// when `use_stdin` is enabled (avoiding Snap `/tmp` temp-file sandbox restrictions).
    #[must_use]
    pub fn mr_create(
        &self,
        title: &str,
        description: &str,
        source_branch: &str,
        target_branch: &str,
        is_draft: bool,
        use_stdin: bool,
    ) -> CliCommandSpec {
        let mut args = vec![
            "mr".to_owned(),
            "create".to_owned(),
            "--yes".to_owned(),
            "--title".to_owned(),
            title.to_owned(),
            "--source-branch".to_owned(),
            source_branch.to_owned(),
            "--target-branch".to_owned(),
            target_branch.to_owned(),
        ];
        if is_draft {
            args.push("--draft".to_owned());
        }
        let stdin = if use_stdin {
            args.push("--description".to_owned());
            args.push("-".to_owned());
            Some(description.to_owned())
        } else {
            args.push("--description".to_owned());
            args.push(description.to_owned());
            None
        };
        self.finish(args, stdin)
    }

    /// Builds `glab mr checkout <iid>`.
    #[must_use]
    pub fn mr_checkout(&self, iid: u64) -> CliCommandSpec {
        self.finish(
            vec!["mr".to_owned(), "checkout".to_owned(), iid.to_string()],
            None,
        )
    }

    fn finish(&self, mut args: Vec<String>, stdin: Option<String>) -> CliCommandSpec {
        if let Some(repo) = &self.repo_path {
            args.push("--repo".to_owned());
            args.push(format!("{}/{repo}", self.host));
        }
        CliCommandSpec {
            program: "glab".to_owned(),
            args,
            env: vec![("GITLAB_HOST".to_owned(), self.host.clone())],
            stdin,
        }
    }
}

/// Classifies `glab` stderr messages (`invalid_grant`, `glab auth login`, rate limits).
#[must_use]
pub fn classify_gitlab_cli_error(stderr: &str) -> GitLabErrorKind {
    let lower = stderr.to_ascii_lowercase();
    if lower.contains("invalid_grant")
        || lower.contains("glab auth login")
        || lower.contains("401 unauthorized")
        || lower.contains("token is expired")
    {
        GitLabErrorKind::AuthExpired
    } else if lower.contains("429") || lower.contains("rate limit") {
        GitLabErrorKind::RateLimited
    } else if lower.contains("404") || lower.contains("not found") {
        GitLabErrorKind::NotFound
    } else {
        GitLabErrorKind::Unknown
    }
}

/// Normalizes a `GitLab` merge request JSON object into [`NormalizedPullRequest`].
///
/// Handles both legacy `work_in_progress` and modern `draft` flags.
///
/// # Errors
///
/// Returns [`ForgeError::MalformedResponse`] if required MR fields are missing.
pub fn normalize_gitlab_mr_json(json: &Value) -> Result<NormalizedPullRequest, ForgeError> {
    let number = json_u64(json, "iid")
        .ok_or_else(|| ForgeError::MalformedResponse("missing GitLab MR `iid`".to_owned()))?;
    let title = json_str(json, "title")
        .ok_or_else(|| ForgeError::MalformedResponse("missing GitLab MR `title`".to_owned()))?
        .to_owned();
    let url = json_str(json, "web_url").unwrap_or("").to_owned();

    let raw_state = json_str(json, "state").unwrap_or("opened");
    let state = parse_standard_pr_state(raw_state, raw_state.eq_ignore_ascii_case("merged"));
    let is_draft = json.get("draft").and_then(Value::as_bool) == Some(true)
        || json.get("work_in_progress").and_then(Value::as_bool) == Some(true);

    let head_branch = json_str(json, "source_branch")
        .ok_or_else(|| ForgeError::MalformedResponse("missing GitLab `source_branch`".to_owned()))?
        .to_owned();
    let base_branch = json_str(json, "target_branch")
        .ok_or_else(|| ForgeError::MalformedResponse("missing GitLab `target_branch`".to_owned()))?
        .to_owned();
    let head_sha = json_str(json, "sha")
        .or_else(|| json.get("diff_refs").and_then(|d| json_str(d, "head_sha")))
        .map(str::to_owned);

    let mergeability = if json.get("has_conflicts").and_then(Value::as_bool) == Some(true) {
        PullRequestMergeability::Conflicting
    } else {
        match json_str(json, "detailed_merge_status").or_else(|| json_str(json, "merge_status")) {
            Some("mergeable" | "can_be_merged") => PullRequestMergeability::Mergeable,
            Some("conflict" | "cannot_be_merged" | "broken_status") => {
                PullRequestMergeability::Conflicting
            }
            _ => PullRequestMergeability::Unknown,
        }
    };

    let checks_state = match json
        .get("head_pipeline")
        .and_then(|p| json_str(p, "status"))
    {
        Some("success") => PullRequestChecksState::Passing,
        Some("failed" | "canceled" | "cancelled") => PullRequestChecksState::Failing,
        _ => PullRequestChecksState::Pending,
    };

    Ok(NormalizedPullRequest {
        number,
        title,
        url,
        state,
        is_draft,
        head_branch,
        base_branch,
        head_sha,
        mergeability,
        checks_state,
    })
}
