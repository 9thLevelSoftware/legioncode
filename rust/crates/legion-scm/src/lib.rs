#![forbid(unsafe_code)]
//! Source-control forge integrations, issue trackers, checkpoint diff/rollback primitives, and rate-limit policies for `LegionCode`.
//!
//! ```
//! use legion_scm::core::discovery::parse_git_remote_url;
//! use legion_scm::forges::linear::{build_linear_api_key_request, LINEAR_GRAPHQL_URL};
//!
//! let remote = parse_git_remote_url("git@github.com:9thlevelsoftware/legioncode.git")?;
//! assert_eq!(remote.full_name, "9thlevelsoftware/legioncode");
//!
//! let req = build_linear_api_key_request("lin_api_example", "{ viewer { id } }", None)?;
//! assert_eq!(req.url, LINEAR_GRAPHQL_URL);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

pub mod checkpoints;
pub mod core;
pub mod forges;
pub mod git_clone_progress;
pub mod git_poll_queries;
pub mod git_ref_cache;
pub mod live_client;
mod live_forge_ops;
pub mod live_git_cli;
pub mod live_git_perf;
pub mod live_http;
pub mod workspace_folders;
pub mod workspace_folders_parse;
pub mod workspace_folders_scanner;
pub mod worktree_guards;

pub use crate::checkpoints::diff_query::{
    CheckpointDiffQuery, ComputedDiff, DiffFileInput, DiffQueryOptions,
};
pub use crate::checkpoints::rollback::{
    GitRestorePlan, RollbackCoordinator, RollbackIntent, VerifiedRollbackIntent,
};
pub use crate::checkpoints::store::{CheckpointDiffBlob, CheckpointStore, ThreadCheckpointSummary};
pub use crate::checkpoints::{CheckpointError, checkpoint_ref_for_turn, parse_checkpoint_ref};
pub use crate::core::CoreError;
pub use crate::core::discovery::{ParsedRemote, RemoteTransport, parse_git_remote_url};
pub use crate::core::pr_helpers::{
    CheckEntry, MergeCommitMessage, MergeMessageInput, RawCheckState, build_merge_commit_message,
    dedupe_checks, remove_agent_credits, rollup_check_statuses, sort_checks,
};
pub use crate::core::rate_limit::{
    QuotaSnapshot, RateLimitDecision, RateLimitPermit, RateLimitUpdate, SourceControlRateLimit,
};
pub use crate::core::revisions::{
    DiffFileFingerprint, DiffFileRevisionTracker, RevisionMatch, sha256_hex,
};
pub use crate::forges::{CliCommandSpec, ForgeError, HttpRequestSpec, NormalizedPullRequest};
pub use crate::live_client::{
    CreatePullRequestParams, ForgeClientTarget, LinearIssueDetails, LiveForgeClient, MergeMethod,
    MergePullRequestOutcome, MergePullRequestParams,
};
pub use crate::live_git_cli::LiveCliOutput;
pub use crate::live_http::{LiveClientError, LiveHttpResponse};
