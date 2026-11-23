//! Multi-account issue tracker bindings, event references, and list filters (`issue.ts`).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ids::{IdError, IssueTrackerAccountId, IssueTrackerKind, ProjectId};

/// Maximum project IDs allowed in `IssueListInput.exclude_project_ids`.
pub const ISSUE_LIST_MAX_EXCLUDED_PROJECTS: usize = 1_000;

/// Maximum additional scopes stored on one [`IssueTrackerProjectBinding`].
pub const MAX_ISSUE_TRACKER_ADDITIONAL_SCOPES: usize = 250;

/// Re-exported error type for validating IDs inside issue tracker references.
pub type IssueRefError = IdError;

/// Reference identifying an issue inside a standalone or forge tracker (`IssueRef`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueRef {
    /// Project whose credentials serve reads/writes.
    pub project_id: ProjectId,
    /// Issue tracker kind (`github`, `linear`, ...).
    pub tracker: IssueTrackerKind,
    /// Optional tracker hostname when not the public default host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// Tracker container scope (`owner/repo`, Linear team key, etc.).
    pub scope: String,
    /// Native issue key (`#123`, `ENG-42`, ...).
    pub key: String,
}

/// Scope reference without an issue key (`IssueScopeRef`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueScopeRef {
    /// Project whose credentials serve reads/writes.
    pub project_id: ProjectId,
    /// Issue tracker kind.
    pub tracker: IssueTrackerKind,
    /// Optional tracker hostname.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// Tracker container scope.
    pub scope: String,
}

/// Entity kind of an [`IssueEventReference`] (`IssueEventReference.kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IssueEventReferenceKind {
    /// Cross-referenced from another issue.
    Issue,
    /// Cross-referenced from a pull request or merge request.
    PullRequest,
}

/// Source issue or change request recorded on a `referenced` issue event (`IssueEvent.reference`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueEventReference {
    /// Repository containing the referencing item.
    pub repository: String,
    /// Numeric issue or pull-request identifier.
    pub number: u32,
    /// Whether the reference came from an issue or pull request.
    pub kind: IssueEventReferenceKind,
    /// Web URL opening the referencing item.
    pub url: String,
}

/// Issue listing state filter (`IssueListState`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IssueListState {
    /// Both open and closed issues.
    All,
    /// Open issues only.
    Open,
    /// Closed issues only.
    Closed,
}

/// Relationship filter on issue listings (`IssueInvolvement`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IssueInvolvement {
    /// Every issue in scope.
    All,
    /// Issues assigned to the viewer.
    Assigned,
    /// Issues authored by the viewer.
    Authored,
    /// Issues mentioning the viewer.
    Mentioned,
}

/// Sort order for `issues.list` (`IssueListSort`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IssueListSort {
    /// Most recently updated first.
    Updated,
    /// Most recently created first.
    Created,
}

/// Cursor map keyed `"<host> <scope>"` (`IssueListCursors`).
pub type IssueListCursors = BTreeMap<String, String>;

/// Input payload for `issues.list` (`IssueListInput`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueListInput {
    /// Open/closed/all category filter.
    pub state: IssueListState,
    /// Optional involvement filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub involvement: Option<IssueInvolvement>,
    /// Optional single-project scope filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<ProjectId>,
    /// Projects excluded from the listing (up to [`ISSUE_LIST_MAX_EXCLUDED_PROJECTS`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclude_project_ids: Option<Vec<ProjectId>>,
    /// Narrow listing to one host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// Page size limit (`1..=200`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Pagination cursors from an earlier slice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursors: Option<IssueListCursors>,
    /// Whether to refresh tracker metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh: Option<bool>,
    /// Free-text query passed to trackers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// Required label names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<String>>,
    /// Assignee login filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    /// Sort order (`updated` or `created`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort: Option<IssueListSort>,
}

/// Project binding to one or more standalone tracker scopes (`IssueTrackerProjectBinding`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueTrackerProjectBinding {
    /// Standalone tracker kind.
    pub tracker: IssueTrackerKind,
    /// Saved tracker account ID; absent for the tracker's first account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<IssueTrackerAccountId>,
    /// Primary container scope where new issues are filed.
    pub scope: String,
    /// Extra scopes included alongside `scope` (up to [`MAX_ISSUE_TRACKER_ADDITIONAL_SCOPES`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_scopes: Option<Vec<String>>,
}

/// Returns all scopes associated with `binding` in priority order (`scope` followed by `additional_scopes`).
#[must_use]
pub fn issue_tracker_binding_scopes(binding: &IssueTrackerProjectBinding) -> Vec<&str> {
    let extra_len = binding.additional_scopes.as_ref().map_or(0, Vec::len);
    let mut out = Vec::with_capacity(1 + extra_len);
    out.push(binding.scope.as_str());
    if let Some(extra) = &binding.additional_scopes {
        out.extend(extra.iter().map(String::as_str));
    }
    out
}

/// One readable container offered when binding a project (`IssueTrackerScope`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueTrackerScope {
    /// Container key.
    pub key: String,
    /// Container display name.
    pub name: String,
}

/// Credential UI metadata on a standalone tracker descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueTrackerCredentialMeta {
    /// Input label (`"Personal API key"`, `"API token"`, ...).
    pub label: String,
    /// Help URL for minting credentials, or `None`.
    pub help_url: Option<String>,
}

/// Connection descriptor and status for one standalone tracker account (`IssueTrackerConnection`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueTrackerConnection {
    /// Standalone tracker kind.
    pub tracker: IssueTrackerKind,
    /// Human-readable tracker display name.
    pub display_name: String,
    /// Tracker hostname served on.
    pub host: String,
    /// Singular noun for scopes (`team`, `project`, ...).
    pub scope_noun: String,
    /// Credential prompt metadata.
    pub credential: IssueTrackerCredentialMeta,
    /// Account identifier; absent for the tracker's first account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<IssueTrackerAccountId>,
    /// Whether a credential is saved.
    pub connected: bool,
    /// Signed-in account label, or `None` when disconnected/rejected.
    pub account: Option<String>,
    /// Rejection or connectivity detail, or `None`.
    pub detail: Option<String>,
    /// Workspace identifier reached by the saved credential, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// Available scopes for binding projects.
    pub scopes: Vec<IssueTrackerScope>,
}

/// Snapshot returned by `issueTrackers.list` (`IssueTrackerConnectionList`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueTrackerConnectionList {
    /// First saved account per standalone tracker.
    pub trackers: Vec<IssueTrackerConnection>,
    /// Additional saved accounts beyond each tracker's first, oldest first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accounts: Option<Vec<IssueTrackerConnection>>,
    /// Project bindings using each tracker's first account.
    pub bindings: BTreeMap<ProjectId, IssueTrackerProjectBinding>,
    /// Project bindings using added accounts (absent from older servers).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_bindings: Option<BTreeMap<ProjectId, IssueTrackerProjectBinding>>,
}

/// Request payload for `issueTrackers.connect` (`IssueTrackerConnectInput`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueTrackerConnectInput {
    /// Standalone tracker kind.
    pub tracker: IssueTrackerKind,
    /// Target account ID when updating or disconnecting a specific added account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<IssueTrackerAccountId>,
    /// When `true`, saves the credential as a new additional account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub add_account: Option<bool>,
    /// Secret credential token, or `None` to disconnect.
    pub token: Option<String>,
}

/// Request payload for `issueTrackers.bindProject` (`IssueTrackerBindProjectInput`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueTrackerBindProjectInput {
    /// Target project ID.
    pub project_id: ProjectId,
    /// Scope binding, or `None` to unbind.
    pub binding: Option<IssueTrackerProjectBinding>,
    /// Account whose binding is removed when unbinding (`binding == None`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<IssueTrackerAccountId>,
}

/// Failure reason on [`IssueTrackerConnectionError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IssueTrackerConnectionFailureReason {
    /// Tracker kind is unknown.
    UnknownTracker,
    /// Credentials were rejected by the tracker API.
    Rejected,
    /// Transport or persistence failure.
    Failed,
}

/// Error returned when connecting or binding a standalone tracker fails (`IssueTrackerConnectionError`).
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[error("{tracker} connection failed: {detail}")]
pub struct IssueTrackerConnectionError {
    /// Standalone tracker kind.
    pub tracker: IssueTrackerKind,
    /// Structured failure reason.
    pub reason: IssueTrackerConnectionFailureReason,
    /// Safe user-facing detail message.
    pub detail: String,
}
