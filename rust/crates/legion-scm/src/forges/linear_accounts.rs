//! Multi-account `Linear` cursor/viewer scoping (`#39179`), team-bound issue creation
//! selection (`#40299`), and overlapping issue + viewer GraphQL reads (`#39801`).

use std::collections::HashSet;

use serde_json::Value;

use crate::forges::linear::{LinearAuth, build_linear_graphql_request};
use crate::forges::{ForgeError, HttpRequestSpec};

/// Project tracker binding paired with optional secondary `Linear` account ID (`#39179`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinearAccountBinding {
    ///Owning project ID.
    pub project_id: String,
    /// Tracker identifier (`"linear"`, `"github"`, etc.).
    pub tracker: String,
    /// Canonical tracker host (`"linear.app"`).
    pub host: String,
    /// Team key or repository scope (original casing preserved).
    pub scope: String,
    /// Optional non-primary account identifier (`None` for first/legacy account).
    pub account_id: Option<String>,
}

/// Bound project scope entry evaluated when selecting which `Linear` team to file into (`#40299`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundIssueProjectScope {
    /// Owning project ID.
    pub project_id: String,
    /// Tracker identifier (`"linear"`).
    pub tracker: String,
    /// Canonical tracker host (`"linear.app"`).
    pub host: String,
    /// Team key (`"ENG"`, `"DESIGN"`, etc.).
    pub scope: String,
}

/// Builds a secret storage key for primary (`None`) or secondary (`Some(id)`) tracker accounts (`#39179`).
#[must_use]
pub fn issue_tracker_secret_name(tracker: &str, account_id: Option<&str>) -> String {
    match account_id.map(str::trim).filter(|s| !s.is_empty()) {
        None => format!("issueTracker:{tracker}"),
        Some(id) => format!("issueTracker:{tracker}:{id}"),
    }
}

/// Builds the list cursor key preserving leading `<host>` grouping across multiple accounts (`#39179`).
#[must_use]
pub fn linear_list_cursor_key(host: &str, scope: &str, account_id: Option<&str>) -> String {
    let lower_scope = scope.trim().to_ascii_lowercase();
    match account_id.map(str::trim).filter(|s| !s.is_empty()) {
        None => format!("{host} {lower_scope}"),
        Some(id) => format!("{host} {id}/{lower_scope}"),
    }
}

/// Builds the viewer lookup cache key distinguishing multiple accounts on the same host (`#39179`).
#[must_use]
pub fn linear_viewer_key(host: &str, account_id: Option<&str>) -> String {
    match account_id.map(str::trim).filter(|s| !s.is_empty()) {
        None => host.to_owned(),
        Some(id) => format!("{host} {id}"),
    }
}

/// Returns `true` only when every configured account on a shared host failed unusable and zero
/// succeeded, preventing one bad secondary account from marking the whole host unconfigured (`#39179`).
#[must_use]
pub fn are_all_host_accounts_unusable<
    S1: ::std::hash::BuildHasher,
    S2: ::std::hash::BuildHasher,
>(
    host_account_keys: &HashSet<String, S1>,
    unusable_account_keys: &HashSet<String, S2>,
    any_succeeded_on_host: bool,
) -> bool {
    !any_succeeded_on_host
        && !host_account_keys.is_empty()
        && host_account_keys
            .iter()
            .all(|key| unusable_account_keys.contains(key))
}

/// Computes account resolution order when resolving a standalone `Linear` issue URL whose team key
/// may exist in multiple workspaces (`#39179`). Exact scope + preferred project comes first.
#[must_use]
pub fn select_standalone_url_account_order(
    on_tracker: &[LinearAccountBinding],
    preferred_project_id: Option<&str>,
    link_scope: &str,
) -> Vec<Option<String>> {
    let target_scope = link_scope.trim().to_ascii_lowercase();
    let exact: Vec<&LinearAccountBinding> = on_tracker
        .iter()
        .filter(|b| b.scope.to_ascii_lowercase() == target_scope)
        .collect();
    let all_refs: Vec<&LinearAccountBinding> = on_tracker.iter().collect();

    let mut ordered = Vec::new();
    push_preferred_first(&exact, preferred_project_id, &mut ordered);
    push_preferred_first(&all_refs, preferred_project_id, &mut ordered);
    ordered
}

fn push_preferred_first(
    items: &[&LinearAccountBinding],
    preferred_project_id: Option<&str>,
    out: &mut Vec<Option<String>>,
) {
    if let Some(pref) = preferred_project_id {
        for item in items.iter().filter(|b| b.project_id == pref) {
            if !out.contains(&item.account_id) {
                out.push(item.account_id.clone());
            }
        }
    }
    for item in items {
        if !out.contains(&item.account_id) {
            out.push(item.account_id.clone());
        }
    }
}

/// Checks whether a fetched `Linear` issue's canonical URL workspace matches a pasted link's
/// target workspace (`#39179`).
#[must_use]
pub fn matches_linear_workspace_url(issue_url: &str, target_workspace: &str) -> bool {
    extract_linear_url_workspace(issue_url)
        .is_some_and(|ws| ws.eq_ignore_ascii_case(target_workspace.trim()))
}

fn extract_linear_url_workspace(url: &str) -> Option<&str> {
    let stripped = url
        .trim()
        .strip_prefix("https://linear.app/")
        .or_else(|| url.trim().strip_prefix("http://linear.app/"))?;
    let mut parts = stripped.split('/');
    let workspace = parts.next()?.trim();
    let segment = parts.next()?;
    if workspace.is_empty() || segment != "issue" {
        return None;
    }
    Some(workspace)
}

/// Resolves which bound team scope to target when creating/updating a `Linear` issue (`#40299`).
///
/// Honors explicit `named_scope` case-insensitively before `<SCOPE>-` key prefix or project default.
///
/// # Errors
///
/// Returns [`ForgeError::MalformedResponse`] if `supported` is empty or `named_scope` is not bound
/// on the project.
pub fn resolve_bound_issue_scope<'a>(
    supported: &'a [BoundIssueProjectScope],
    named_scope: Option<&str>,
    issue_key: Option<&str>,
) -> Result<&'a BoundIssueProjectScope, ForgeError> {
    if supported.is_empty() {
        return Err(ForgeError::MalformedResponse(
            "tracker-unsupported: project has no bound issue tracker scopes".to_owned(),
        ));
    }
    if let Some(raw_named) = named_scope.map(str::trim).filter(|s| !s.is_empty()) {
        let named_lower = raw_named.to_ascii_lowercase();
        if let Some(found) = supported
            .iter()
            .find(|c| c.scope.to_ascii_lowercase() == named_lower)
        {
            return Ok(found);
        }
        let mut available = Vec::new();
        for c in supported {
            if !available
                .iter()
                .any(|existing: &String| existing == &c.scope)
            {
                available.push(c.scope.clone());
            }
        }
        return Err(ForgeError::MalformedResponse(format!(
            "This project does not track {raw_named}. Use one of: {}.",
            available.join(", ")
        )));
    }

    if let Some(prefix_key) = issue_key.map(|k| k.trim().to_ascii_lowercase())
        && let Some(found) = supported
            .iter()
            .find(|c| prefix_key.starts_with(&format!("{}-", c.scope.to_ascii_lowercase())))
    {
        return Ok(found);
    }

    Ok(&supported[0])
}

/// Builds a lightweight `Linear` GraphQL request fetching the authenticated viewer `{ id name displayName }` (`#39801`).
///
/// # Errors
///
/// Returns [`ForgeError::InvalidCredentials`] if credentials are empty or contain CR/LF.
pub fn build_linear_viewer_request(auth: &LinearAuth) -> Result<HttpRequestSpec, ForgeError> {
    build_linear_graphql_request(
        auth,
        "query LinearViewer { viewer { id name displayName email } }",
        None,
    )
}

/// Extracts `(viewer_id, viewer_handle)` from a `LinearViewer` GraphQL response (`#39801`).
#[must_use]
pub fn parse_linear_viewer_identity(body_json: &Value) -> Option<(Option<String>, Option<String>)> {
    let viewer = body_json.get("data")?.get("viewer")?.as_object()?;
    let id = viewer
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let handle = viewer
        .get("displayName")
        .or_else(|| viewer.get("name"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    Some((id, handle))
}
