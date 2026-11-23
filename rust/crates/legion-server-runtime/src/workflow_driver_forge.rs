//! Concrete [`WorkflowForgeDriver`] wiring `legion-scm` into `legion-workflows` (`#38882`).

use std::sync::{Arc, Mutex};

use legion_contracts::pull_requests::PullRequestState;
use legion_persistence::project_store::find_project_by_id_or_workspace;
use legion_scm::core::discovery::ParsedRemote;
use legion_scm::forges::github::GitHubEndpoint;
use legion_scm::live_client::{ForgeClientTarget, LiveForgeClient, MergeMethod};
use legion_workflows::traits::{
    BoxFuture, IssueAddress, IssueCreateInput, IssueQuery, PullRequestAddress, PullRequestDetail,
    PullRequestQuery, WorkflowForgeDriver, WorkflowForgeError,
};
use rusqlite::Connection;
use serde_json::{Value, json};

/// Issue & pull-request forge driver backed by project SQLite + `LiveForgeClient`.
#[derive(Clone)]
pub struct ScmWorkflowForgeDriver {
    db: Arc<Mutex<Connection>>,
}

impl ScmWorkflowForgeDriver {
    /// Creates a new forge driver querying workspace remotes via `db`.
    pub fn new(db: Arc<Mutex<Connection>>) -> Self {
        Self { db }
    }

    fn resolve_workspace_root(&self, project_hint: &str) -> Result<String, WorkflowForgeError> {
        let conn = self
            .db
            .lock()
            .map_err(|_| WorkflowForgeError::new("database lock poisoned"))?;
        if let Some(found) = find_project_by_id_or_workspace(&*conn, project_hint)
            .map_err(|e| WorkflowForgeError::new(e.to_string()))?
        {
            return Ok(found.workspace_root);
        }
        if std::path::Path::new(project_hint).is_dir() {
            return Ok(project_hint.to_owned());
        }
        Err(WorkflowForgeError::new(format!(
            "Project `{project_hint}` was not found."
        )))
    }
}

impl WorkflowForgeDriver for ScmWorkflowForgeDriver {
    fn issues_list(
        &self,
        query: IssueQuery,
    ) -> BoxFuture<'_, Result<Vec<Value>, WorkflowForgeError>> {
        Box::pin(async move {
            let _root = self.resolve_workspace_root(&query.project)?;
            Ok(Vec::new())
        })
    }

    fn issues_read(&self, issue: IssueAddress) -> BoxFuture<'_, Result<Value, WorkflowForgeError>> {
        Box::pin(async move {
            match issue {
                IssueAddress::Url { project, url } => {
                    let _root = self.resolve_workspace_root(&project)?;
                    Ok(json!({ "url": url }))
                }
                IssueAddress::Ref {
                    project_id,
                    tracker,
                    host,
                    scope,
                    key,
                } => Ok(json!({
                    "projectId": project_id,
                    "tracker": tracker,
                    "host": host,
                    "scope": scope,
                    "key": key,
                })),
            }
        })
    }

    fn issues_comment(
        &self,
        _issue: IssueAddress,
        _body: String,
    ) -> BoxFuture<'_, Result<(), WorkflowForgeError>> {
        Box::pin(async {
            Err(WorkflowForgeError::new(
                "Issue tracker authentication token is not configured for workflow comments.",
            ))
        })
    }

    fn issues_set_state(
        &self,
        _issue: IssueAddress,
        _state: String,
    ) -> BoxFuture<'_, Result<(), WorkflowForgeError>> {
        Box::pin(async {
            Err(WorkflowForgeError::new(
                "Issue tracker authentication token is not configured for state updates.",
            ))
        })
    }

    fn issues_create(
        &self,
        _input: IssueCreateInput,
    ) -> BoxFuture<'_, Result<Value, WorkflowForgeError>> {
        Box::pin(async {
            Err(WorkflowForgeError::new(
                "Issue tracker authentication token is not configured for creating issues.",
            ))
        })
    }

    fn pull_requests_list(
        &self,
        query: PullRequestQuery,
    ) -> BoxFuture<'_, Result<Vec<Value>, WorkflowForgeError>> {
        Box::pin(async move {
            let root = self.resolve_workspace_root(&query.project)?;
            let client =
                LiveForgeClient::new().map_err(|e| WorkflowForgeError::new(e.to_string()))?;
            let remote = client
                .discover_workspace_remote(std::path::Path::new(&root))
                .await
                .map_err(|e| WorkflowForgeError::new(e.to_string()))?;
            let (target, repo) = remote_to_target(&remote, None)?;
            let prs = client
                .list_pull_requests(&target, &repo, query.state.as_deref())
                .await
                .map_err(|e| WorkflowForgeError::new(e.to_string()))?;
            Ok(prs
                .into_iter()
                .map(|pr| {
                    json!({
                        "projectId": query.project,
                        "host": remote.host,
                        "repository": repo,
                        "number": pr.number,
                        "title": pr.title,
                        "url": pr.url,
                        "state": pr_state_str(pr.state),
                    })
                })
                .collect())
        })
    }

    fn pull_requests_read(
        &self,
        pull_request: PullRequestAddress,
    ) -> BoxFuture<'_, Result<PullRequestDetail, WorkflowForgeError>> {
        Box::pin(async move {
            let (project_id, repo_opt, number) = match pull_request {
                PullRequestAddress::Ref {
                    project_id,
                    repository,
                    number,
                    ..
                } => (project_id, Some(repository), number),
                PullRequestAddress::Url { project, url } => {
                    let parsed = url
                        .rsplit('/')
                        .next()
                        .and_then(|s| s.parse::<i64>().ok())
                        .ok_or_else(|| {
                            WorkflowForgeError::new(format!(
                                "Pull request URL `{url}` does not end with a number."
                            ))
                        })?;
                    (project, None, parsed)
                }
            };
            let root = self.resolve_workspace_root(&project_id)?;
            let client =
                LiveForgeClient::new().map_err(|e| WorkflowForgeError::new(e.to_string()))?;
            let remote = client
                .discover_workspace_remote(std::path::Path::new(&root))
                .await
                .map_err(|e| WorkflowForgeError::new(e.to_string()))?;
            let (target, repo) = remote_to_target(&remote, repo_opt.as_deref())?;
            let pr = client
                .fetch_pull_request(
                    &target,
                    &repo,
                    u64::try_from(number).map_err(|_| {
                        WorkflowForgeError::new("Pull request number must be positive.")
                    })?,
                )
                .await
                .map_err(|e| WorkflowForgeError::new(e.to_string()))?;
            Ok(PullRequestDetail {
                project_id,
                host: remote.host,
                repository: repo,
                number,
                title: pr.title,
                url: pr.url,
                state: pr_state_str(pr.state).to_owned(),
                mergeability: "unknown".to_owned(),
                checks: Vec::new(),
            })
        })
    }

    fn pull_requests_comment(
        &self,
        _pull_request: PullRequestAddress,
        _body: String,
    ) -> BoxFuture<'_, Result<(), WorkflowForgeError>> {
        Box::pin(async {
            Err(WorkflowForgeError::new(
                "Pull request write token is required for workflow comments.",
            ))
        })
    }

    fn pull_requests_act(
        &self,
        pull_request: PullRequestAddress,
        action: String,
        merge_method: Option<String>,
    ) -> BoxFuture<'_, Result<(), WorkflowForgeError>> {
        Box::pin(async move {
            if action != "merge" {
                return Err(WorkflowForgeError::new(format!(
                    "Pull request action `{action}` requires forge token credentials."
                )));
            }
            let (project_id, repo_opt, number) = match pull_request {
                PullRequestAddress::Ref {
                    project_id,
                    repository,
                    number,
                    ..
                } => (project_id, Some(repository), number),
                PullRequestAddress::Url { .. } => {
                    return Err(WorkflowForgeError::new(
                        "Pull request merge requires a structured repository reference.",
                    ));
                }
            };
            let root = self.resolve_workspace_root(&project_id)?;
            let client =
                LiveForgeClient::new().map_err(|e| WorkflowForgeError::new(e.to_string()))?;
            let remote = client
                .discover_workspace_remote(std::path::Path::new(&root))
                .await
                .map_err(|e| WorkflowForgeError::new(e.to_string()))?;
            let (target, repo) = remote_to_target(&remote, repo_opt.as_deref())?;
            let method = match merge_method.as_deref() {
                Some("rebase") => MergeMethod::Rebase,
                Some("merge") => MergeMethod::Merge,
                _ => MergeMethod::Squash,
            };
            client
                .merge_pull_request(
                    &target,
                    &legion_scm::live_client::MergePullRequestParams {
                        repo,
                        number: u64::try_from(number).unwrap_or(1),
                        commit_title: None,
                        commit_message: None,
                        method,
                        strip_agent_credits: true,
                    },
                )
                .await
                .map(|_| ())
                .map_err(|e| WorkflowForgeError::new(e.to_string()))
        })
    }
}

const fn pr_state_str(state: PullRequestState) -> &'static str {
    match state {
        PullRequestState::Open => "open",
        PullRequestState::Closed => "closed",
        PullRequestState::Merged => "merged",
    }
}

fn remote_to_target(
    remote: &ParsedRemote,
    override_repo: Option<&str>,
) -> Result<(ForgeClientTarget, String), WorkflowForgeError> {
    let repo = override_repo
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map_or_else(|| remote.full_name.clone(), str::to_owned);
    let token = std::env::var("GITHUB_TOKEN")
        .or_else(|_| std::env::var("GH_TOKEN"))
        .unwrap_or_default();
    let endpoint = GitHubEndpoint::for_host(&remote.host)
        .map_err(|e| WorkflowForgeError::new(e.to_string()))?;
    Ok((ForgeClientTarget::GitHub { endpoint, token }, repo))
}
