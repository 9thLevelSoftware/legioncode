//! `Azure` `DevOps` CLI adapter (`cmd.exe` bypass via `python.exe`, `@-` stdin escaping, `_git` scoping).

use legion_contracts::pull_requests::{
    PullRequestChecksState, PullRequestMergeability, PullRequestState,
};
use serde_json::Value;

use crate::core::discovery::ParsedRemote;
pub use crate::forges::azure_devops_auth::{
    AzureAuthenticatedViewer, azure_pat_cli_env, build_azure_connection_data_cli_spec,
    build_azure_connection_data_http_request, build_azure_project_list_cli_spec,
    encode_azure_pat_basic_auth, verify_azure_connection_data,
};
use crate::forges::{CliCommandSpec, ForgeError, NormalizedPullRequest, json_str, json_u64};

/// Resolved `Azure` CLI launcher details.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AzureCliLaunch {
    /// Executable path (`<install>\python.exe` when bypassing Windows `az.cmd`, else `az`).
    pub program: String,
    /// Mandatory prefix args before `az` subcommands (e.g. `["-X", "utf8", "-IBm", "azure.cli"]`).
    pub prefix_args: Vec<String>,
    /// Required environment overrides (e.g. `("AZ_INSTALLER", "MSI" | "ZIP")`).
    pub env: Vec<(String, String)>,
}

/// Resolves an `Azure` CLI launcher, bypassing Windows `cmd.exe` (`az.cmd`) when installed
/// with its bundled `<install>\python.exe` and `azure.cli` site-package.
///
/// Recognizes MSI (`<install>\wbin\az.cmd`) and ZIP (`<install>\bin\az.cmd`) layouts.
///
/// ```
/// use legion_scm::forges::azure_devops::resolve_azure_cli_launch;
///
/// let msi = resolve_azure_cli_launch(
///     r"C:\Program Files\Microsoft SDKs\Azure\CLI2\wbin\az.cmd",
///     true,
///     true,
///     true,
/// );
/// assert_eq!(msi.program, r"C:\Program Files\Microsoft SDKs\Azure\CLI2\python.exe");
/// assert_eq!(msi.prefix_args, vec!["-X", "utf8", "-IBm", "azure.cli"]);
/// assert_eq!(msi.env, vec![("AZ_INSTALLER".into(), "MSI".into())]);
/// ```
#[must_use]
pub fn resolve_azure_cli_launch(
    cmd_path: &str,
    is_windows: bool,
    has_python_exe: bool,
    has_azure_cli_pkg: bool,
) -> AzureCliLaunch {
    let trimmed = cmd_path.trim();
    if is_windows && has_python_exe && has_azure_cli_pkg {
        let normalized = trimmed.replace('/', "\\");
        let lower = normalized.to_ascii_lowercase();
        let detected = if lower.ends_with("\\wbin\\az.cmd") {
            let root = &normalized[..normalized.len() - "\\wbin\\az.cmd".len()];
            Some((root, "MSI"))
        } else if lower.ends_with("\\bin\\az.cmd") {
            let root = &normalized[..normalized.len() - "\\bin\\az.cmd".len()];
            Some((root, "ZIP"))
        } else {
            None
        };
        if let Some((install_root, installer)) = detected
            && !install_root.is_empty()
        {
            return AzureCliLaunch {
                program: format!("{install_root}\\python.exe"),
                prefix_args: vec![
                    "-X".to_owned(),
                    "utf8".to_owned(),
                    "-IBm".to_owned(),
                    "azure.cli".to_owned(),
                ],
                env: vec![("AZ_INSTALLER".to_owned(), installer.to_owned())],
            };
        }
    }

    AzureCliLaunch {
        program: if trimmed.is_empty() {
            "az".to_owned()
        } else {
            trimmed.to_owned()
        },
        prefix_args: Vec::new(),
        env: Vec::new(),
    }
}

/// Encodes a CLI text option safely for `az`: values starting with `@` use `--{flag}=@-` + `stdin`
/// so `Azure` CLI does not interpret `@path` as a local file inclusion directive.
#[must_use]
pub fn azure_text_option(flag: &str, value: &str) -> (Vec<String>, Option<String>) {
    let clean_flag = flag.trim_start_matches('-');
    if value.starts_with('@') {
        (vec![format!("--{clean_flag}=@-")], Some(value.to_owned()))
    } else {
        (vec![format!("--{clean_flag}={value}")], None)
    }
}

/// Strips `--detect ...` flags and injects `--organization` and `--project` from [`ParsedRemote`].
#[must_use]
pub fn scope_azure_cli_args(remote: &ParsedRemote, raw_args: &[&str]) -> Vec<String> {
    let mut filtered: Vec<String> = Vec::with_capacity(raw_args.len() + 4);
    let mut skip_next = false;
    for arg in raw_args {
        if skip_next {
            skip_next = false;
            continue;
        }
        if *arg == "--detect" {
            skip_next = true;
            continue;
        }
        if arg.starts_with("--detect=") {
            continue;
        }
        filtered.push((*arg).to_owned());
    }

    if let Some(az) = &remote.azure {
        filtered.push("--organization".to_owned());
        filtered.push(format!("https://dev.azure.com/{}", az.organization));
        filtered.push("--project".to_owned());
        filtered.push(az.project.clone());
    }
    filtered
}

/// Combines [`AzureCliLaunch`], scoped args, and `@-` `stdin` payload into a [`CliCommandSpec`].
#[must_use]
pub fn build_azure_pr_create_command(
    launch: &AzureCliLaunch,
    remote: &ParsedRemote,
    title: &str,
    description: &str,
    source_branch: &str,
    target_branch: &str,
    draft: bool,
) -> CliCommandSpec {
    let mut args = launch.prefix_args.clone();
    let base = vec![
        "repos".to_owned(),
        "pr".to_owned(),
        "create".to_owned(),
        "--output".to_owned(),
        "json".to_owned(),
        "--source-branch".to_owned(),
        strip_refs_heads(source_branch).to_owned(),
        "--target-branch".to_owned(),
        strip_refs_heads(target_branch).to_owned(),
        "--title".to_owned(),
        title.to_owned(),
    ];
    args.extend(base);
    if draft {
        args.push("--draft".to_owned());
        args.push("true".to_owned());
    }
    if let Some(az) = &remote.azure {
        args.push("--repository".to_owned());
        args.push(az.repository.clone());
    }
    let scoped_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut scoped = scope_azure_cli_args(remote, &scoped_refs);

    let (desc_args, stdin) = azure_text_option("description", description);
    scoped.extend(desc_args);

    CliCommandSpec {
        program: launch.program.clone(),
        args: scoped,
        env: launch.env.clone(),
        stdin,
    }
}

/// Strips a leading `refs/heads/` prefix from an `Azure` `DevOps` branch ref.
#[must_use]
pub fn strip_refs_heads(ref_name: &str) -> &str {
    ref_name.strip_prefix("refs/heads/").unwrap_or(ref_name)
}

/// Normalizes an `Azure` `DevOps` `az repos pr` JSON payload into [`NormalizedPullRequest`].
///
/// # Errors
///
/// Returns [`ForgeError::MalformedResponse`] if required `pullRequestId`, `title`, or branches
/// are missing.
pub fn normalize_azure_pr_json(json: &Value) -> Result<NormalizedPullRequest, ForgeError> {
    let number = json_u64(json, "pullRequestId")
        .ok_or_else(|| ForgeError::MalformedResponse("missing Azure `pullRequestId`".to_owned()))?;
    let title = json_str(json, "title")
        .ok_or_else(|| ForgeError::MalformedResponse("missing Azure PR `title`".to_owned()))?
        .to_owned();
    let url = json_str(json, "url").unwrap_or("").to_owned();

    let state = match json_str(json, "status")
        .unwrap_or("active")
        .to_ascii_lowercase()
        .as_str()
    {
        "completed" => PullRequestState::Merged,
        "abandoned" => PullRequestState::Closed,
        _ => PullRequestState::Open,
    };
    let is_draft = json
        .get("isDraft")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let head_branch = strip_refs_heads(json_str(json, "sourceRefName").ok_or_else(|| {
        ForgeError::MalformedResponse("missing Azure `sourceRefName`".to_owned())
    })?)
    .to_owned();
    let base_branch = strip_refs_heads(json_str(json, "targetRefName").ok_or_else(|| {
        ForgeError::MalformedResponse("missing Azure `targetRefName`".to_owned())
    })?)
    .to_owned();
    let head_sha = json
        .get("lastMergeSourceCommit")
        .and_then(|c| json_str(c, "commitId"))
        .map(str::to_owned);

    let mergeability = match json_str(json, "mergeStatus").unwrap_or("") {
        "succeeded" => PullRequestMergeability::Mergeable,
        "conflicts" | "rejectedByPolicy" | "failure" => PullRequestMergeability::Conflicting,
        _ => PullRequestMergeability::Unknown,
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
        checks_state: PullRequestChecksState::Pending,
    })
}

/// Parsed pull-request reference extracted from an `Azure` `DevOps` URL (`#40436`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AzurePullRequestUrlRef {
    /// Organization segment (`dev.azure.com/{org}` or `{org}.visualstudio.com`).
    pub organization: String,
    /// Optional project segment (`None` for project-less URLs accepted in `#40436`).
    pub project: Option<String>,
    /// Git repository slug after `/_git/`.
    pub repository: String,
    /// Numeric pull request ID.
    pub pull_request_id: u64,
}

/// Parses project-scoped and project-less `Azure` `DevOps` pull-request URLs (`#40436`), including
/// `dev.azure.com/{org}[/{project}]/_git/{repo}/pullrequest/{id}` and
/// `{org}.visualstudio.com[/DefaultCollection][/{project}]/_git/{repo}/pullrequest/{id}`.
#[must_use]
pub fn parse_azure_change_request_url(raw_url: &str) -> Option<AzurePullRequestUrlRef> {
    let trimmed = raw_url.trim();
    let without_scheme = trimmed.strip_prefix("https://")?;
    let clean = without_scheme.split(['?', '#']).next()?;
    let (host, path) = clean.split_once('/')?;
    let host_lower = host.to_ascii_lowercase();
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let git_idx = segments.iter().position(|&s| s == "_git")?;
    if git_idx + 3 >= segments.len() || !segments[git_idx + 2].eq_ignore_ascii_case("pullrequest") {
        return None;
    }
    let repository = segments[git_idx + 1].to_owned();
    let pull_request_id = segments[git_idx + 3]
        .parse::<u64>()
        .ok()
        .filter(|&n| n > 0)?;

    if host_lower == "dev.azure.com" {
        if git_idx == 1 {
            return Some(AzurePullRequestUrlRef {
                organization: segments[0].to_owned(),
                project: None,
                repository,
                pull_request_id,
            });
        }
        if git_idx == 2 {
            return Some(AzurePullRequestUrlRef {
                organization: segments[0].to_owned(),
                project: Some(segments[1].to_owned()),
                repository,
                pull_request_id,
            });
        }
        return None;
    }

    if let Some(org) = host_lower.strip_suffix(".visualstudio.com")
        && !org.is_empty()
    {
        let prefix = &segments[..git_idx];
        let project_slice = match prefix.first().copied() {
            Some(first) if first.eq_ignore_ascii_case("DefaultCollection") => &prefix[1..],
            _ => prefix,
        };
        if project_slice.len() > 1 {
            return None;
        }
        return Some(AzurePullRequestUrlRef {
            organization: org.to_owned(),
            project: project_slice.first().map(|p| (*p).to_owned()),
            repository,
            pull_request_id,
        });
    }
    None
}

/// Selects the work-item text field (`Microsoft.VSTS.TCM.ReproSteps` for Bugs without
/// `System.Description`, else `System.Description`) (`#39738`).
#[must_use]
pub fn azure_work_item_body_field(
    work_item_type: Option<&str>,
    system_description: Option<&str>,
    repro_steps: Option<&str>,
) -> &'static str {
    let has_desc = system_description.is_some_and(|s| !s.trim().is_empty());
    let has_repro = repro_steps.is_some_and(|s| !s.trim().is_empty());
    let is_bug = work_item_type
        .map(str::trim)
        .is_some_and(|t| t.eq_ignore_ascii_case("bug"));
    if !has_desc && (is_bug || has_repro) {
        "Microsoft.VSTS.TCM.ReproSteps"
    } else {
        "System.Description"
    }
}

/// Builds `az repos pr create` args including `--draft true` when `draft` is enabled (`#39772`).
#[must_use]
pub fn build_azure_pr_create_args(
    remote: &ParsedRemote,
    source_branch: &str,
    target_branch: &str,
    title: &str,
    body_file: &str,
    draft: bool,
) -> (Vec<String>, Option<String>) {
    let (mut title_args, stdin) = azure_text_option("title", title);
    let mut raw_args: Vec<&str> = vec![
        "repos",
        "pr",
        "create",
        "--output",
        "json",
        "--source-branch",
        source_branch,
        "--target-branch",
        target_branch,
    ];
    let desc_flag = format!("--description=@{body_file}");
    raw_args.push(&desc_flag);
    if draft {
        raw_args.push("--draft");
        raw_args.push("true");
    }
    let mut scoped = scope_azure_cli_args(remote, &raw_args);
    scoped.append(&mut title_args);
    (scoped, stdin)
}
