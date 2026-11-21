//! Invocation-budgeted git queries for remote polling (`2c01259fb4`), branch-list reloads
//! (`d57b83d85e`, `38d27db746`), and branch PR lookup (`2af435185e`, `c1464d6774`).

use std::path::Path;

use crate::git_ref_cache::{EpochRefSnapshotCache, PrLookupGitMemo};
use crate::live_git_cli::execute_raw_bytes;
use crate::live_http::LiveClientError;

/// Consolidated `HEAD`, upstream, and remote snapshot read on a remote status poll (`2c01259fb4`, `c1464d6774`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemotePollMetadata {
    /// Full symbolic ref if on branch (`refs/heads/...`), or `None` if detached/unborn.
    pub head_ref: Option<String>,
    /// Upstream branch ref (`origin/main`), if configured and valid.
    pub upstream_ref: Option<String>,
    /// Upstream remote name (`origin`), if configured.
    pub upstream_remote: Option<String>,
    /// True if at least one git remote ref or configured remote exists (`c1464d6774`).
    pub has_primary_remote: bool,
    /// Number of git subprocesses executed to gather this snapshot.
    pub git_invocations: usize,
}

/// Reads `HEAD`, upstream tracking ref, and remote presence in a single `git for-each-ref`
/// invocation when checked out on a committed branch (`2c01259fb4`), falling back to `git remote`
/// only when `refs/remotes` is empty (`c1464d6774`).
pub async fn read_remote_poll_metadata(
    repo_dir: &Path,
    base_env: &[(String, String)],
) -> Result<RemotePollMetadata, LiveClientError> {
    let mut invocations = 1usize;
    let out = execute_raw_bytes(
        "git",
        &[
            "for-each-ref".to_owned(),
            "--format=%(HEAD)%09%(refname)%09%(upstream:short)%09%(upstream:remotename)".to_owned(),
            "refs/heads".to_owned(),
            "refs/remotes".to_owned(),
        ],
        base_env,
        &[],
        None,
        Some(repo_dir),
    )
    .await?;
    if !out.success {
        return Err(LiveClientError::CliExit {
            program: "git".to_owned(),
            code: out.status_code,
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        });
    }
    let parsed = parse_single_pass_for_each_ref(&String::from_utf8_lossy(&out.stdout));
    let has_primary_remote = if parsed.has_any_remote_ref {
        true
    } else {
        invocations += 1;
        let rem = execute_raw_bytes(
            "git",
            &["remote".to_owned()],
            base_env,
            &[],
            None,
            Some(repo_dir),
        )
        .await?;
        !String::from_utf8_lossy(&rem.stdout).trim().is_empty()
    };

    Ok(RemotePollMetadata {
        head_ref: parsed.head_ref,
        upstream_ref: parsed.upstream_ref,
        upstream_remote: parsed.upstream_remote,
        has_primary_remote,
        git_invocations: invocations,
    })
}

struct ParsedPollRefs {
    head_ref: Option<String>,
    upstream_ref: Option<String>,
    upstream_remote: Option<String>,
    has_any_remote_ref: bool,
}

fn parse_single_pass_for_each_ref(stdout: &str) -> ParsedPollRefs {
    let mut head_ref = None;
    let mut upstream_ref = None;
    let mut upstream_remote = None;
    let mut has_any_remote_ref = false;

    for line in stdout.lines() {
        let mut cols = line.splitn(4, '\t');
        let Some(head_marker) = cols.next() else {
            continue;
        };
        let Some(refname) = cols.next().map(str::trim) else {
            continue;
        };
        if refname.starts_with("refs/remotes/") {
            has_any_remote_ref = true;
        }
        if head_marker.trim() == "*" && !refname.is_empty() {
            head_ref = Some(refname.to_owned());
            let up = cols.next().unwrap_or("").trim();
            let up_rem = cols.next().unwrap_or("").trim();
            if !up.is_empty() {
                upstream_ref = Some(up.to_owned());
            }
            if !up_rem.is_empty() && up_rem != "." {
                upstream_remote = Some(up_rem.to_owned());
                has_any_remote_ref = true;
            }
        }
    }

    ParsedPollRefs {
        head_ref,
        upstream_ref,
        upstream_remote,
        has_any_remote_ref,
    }
}

/// Snapshot produced by [`reload_branch_list_budgeted`] (`d57b83d85e`, `38d27db746`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchListSnapshot {
    /// Epoch assigned when storing in [`EpochRefSnapshotCache`].
    pub epoch: u64,
    /// Current branch name if attached.
    pub current_branch: Option<String>,
    /// Listed `refs/heads/*` and `refs/remotes/*` short names.
    pub refs: Vec<String>,
    /// Git invocations spent on this reload (always `<= BRANCH_LIST_MAX_GIT_INVOCATIONS`).
    pub git_invocations: usize,
}

/// Reloads branch list snapshots into `cache` in `<= 6` git invocations (`d57b83d85e`) while
/// dropping the previous snapshot immediately (`38d27db746`).
pub async fn reload_branch_list_budgeted(
    repo_dir: &Path,
    cache: &mut EpochRefSnapshotCache<Vec<String>>,
    base_env: &[(String, String)],
) -> Result<BranchListSnapshot, LiveClientError> {
    let mut invocations = 0usize;
    invocations += 1;
    let layout = execute_raw_bytes(
        "git",
        &[
            "rev-parse".to_owned(),
            "--git-common-dir".to_owned(),
            "--show-toplevel".to_owned(),
        ],
        base_env,
        &[],
        None,
        Some(repo_dir),
    )
    .await?;
    let common_dir = String::from_utf8_lossy(&layout.stdout)
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_owned();
    if !layout.success || common_dir.is_empty() {
        return Err(LiveClientError::CliExit {
            program: "git".to_owned(),
            code: layout.status_code,
            stderr: String::from_utf8_lossy(&layout.stderr).trim().to_owned(),
        });
    }

    invocations += 1;
    let list_out = execute_raw_bytes(
        "git",
        &[
            "for-each-ref".to_owned(),
            "--format=%(HEAD)%09%(refname:short)".to_owned(),
            "refs/heads".to_owned(),
            "refs/remotes".to_owned(),
        ],
        base_env,
        &[],
        None,
        Some(repo_dir),
    )
    .await?;
    let mut current_branch = None;
    let mut refs = Vec::new();
    for line in String::from_utf8_lossy(&list_out.stdout).lines() {
        if let Some((head, name)) = line.split_once('\t') {
            let trimmed = name.trim();
            if !trimmed.is_empty() {
                if head.trim() == "*" {
                    current_branch = Some(trimmed.to_owned());
                }
                refs.push(trimmed.to_owned());
            }
        }
    }
    let epoch = cache.replace(&common_dir, refs.clone());
    Ok(BranchListSnapshot {
        epoch,
        current_branch,
        refs,
        git_invocations: invocations,
    })
}

/// Branch PR git lookup state gathered via [`PrLookupGitMemo`] (`2af435185e`, `c1464d6774`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchPrGitContext {
    /// Configured remote names (`git remote`).
    pub remotes: Vec<String>,
    /// Selected remote URL if a primary remote exists.
    pub remote_url: Option<String>,
    /// Git invocations spent on this lookup (always `<= BRANCH_PR_LOOKUP_MAX_GIT_INVOCATIONS`).
    pub git_invocations: usize,
}

/// Gathers git remote/config context for a branch PR lookup in `<= 9` invocations (`2af435185e`),
/// skipping remote URL / config reads completely when zero remotes exist (`c1464d6774`).
pub async fn resolve_branch_pr_git_context(
    repo_dir: &Path,
    branch: &str,
    memo: &mut PrLookupGitMemo,
    base_env: &[(String, String)],
) -> Result<BranchPrGitContext, LiveClientError> {
    let mut invocations = 0usize;
    let remotes = if memo.can_reuse()
        && let Some(cached) = &memo.remotes
    {
        cached.clone()
    } else {
        invocations += 1;
        let out = execute_raw_bytes(
            "git",
            &["remote".to_owned()],
            base_env,
            &[],
            None,
            Some(repo_dir),
        )
        .await?;
        let names: Vec<String> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        memo.remotes = Some(names.clone());
        names
    };

    if remotes.is_empty() {
        return Ok(BranchPrGitContext {
            remotes,
            remote_url: None,
            git_invocations: invocations,
        });
    }

    let remote_key = format!("branch.{branch}.remote");
    if !memo.can_reuse() || !memo.config_values.contains_key(&remote_key) {
        invocations += 1;
        let cfg = execute_raw_bytes(
            "git",
            &["config".to_owned(), "--get".to_owned(), remote_key.clone()],
            base_env,
            &[],
            None,
            Some(repo_dir),
        )
        .await?;
        let val = cfg
            .success
            .then(|| String::from_utf8_lossy(&cfg.stdout).trim().to_owned())
            .filter(|s| !s.is_empty());
        memo.config_values.insert(remote_key.clone(), val);
    }

    let primary_remote = memo
        .config_values
        .get(&remote_key)
        .and_then(|v| v.as_deref())
        .filter(|r| *r != ".")
        .map(str::to_owned)
        .or_else(|| {
            remotes
                .iter()
                .find(|r| r.as_str() == "origin")
                .or_else(|| remotes.first())
                .cloned()
        });

    let remote_url = if let Some(remote_name) = primary_remote {
        if memo.can_reuse()
            && let Some(cached_url) = memo.remote_urls.get(&remote_name)
        {
            cached_url.clone()
        } else {
            invocations += 1;
            let url_out = execute_raw_bytes(
                "git",
                &[
                    "remote".to_owned(),
                    "get-url".to_owned(),
                    remote_name.clone(),
                ],
                base_env,
                &[],
                None,
                Some(repo_dir),
            )
            .await?;
            let url = url_out
                .success
                .then(|| String::from_utf8_lossy(&url_out.stdout).trim().to_owned())
                .filter(|s| !s.is_empty());
            memo.remote_urls.insert(remote_name, url.clone());
            url
        }
    } else {
        None
    };

    Ok(BranchPrGitContext {
        remotes,
        remote_url,
        git_invocations: invocations,
    })
}
