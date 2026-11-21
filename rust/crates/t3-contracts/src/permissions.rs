//! RPC scope authorization contracts and multi-scope grant enforcement.
//!
//! Mirrors `packages/contracts/src/auth.ts:31-143`,
//! `packages/contracts/src/clientRpcPermissions.ts`, and
//! `packages/contracts/src/settings.ts:2001-2047`, fixing BUG-17:
//! - [`required_scopes_for_rpc`] and [`required_scopes_for_rpc_json`] return a
//!   typed `Result<Vec<EnvironmentScope>, GuardEvalError>` on malformed JSON or
//!   invalid dynamic payloads rather than throwing a synchronous defect.
//! - [`grants_all_scopes`] verifies *every* required scope against the session
//!   (instead of only checking index `0`).
//!
//! # Examples
//!
//! ```
//! use serde_json::json;
//! use t3_contracts::permissions::{
//!     EnvironmentScope, SessionGrantInput, WsMethod, grants_all_scopes,
//!     required_scopes_for_rpc, required_scopes_for_rpc_json,
//! };
//!
//! let payload = json!({
//!     "cwd": "/repo",
//!     "reference": "#42",
//!     "mode": "worktree",
//!     "threadId": "thread-1",
//! });
//! let required = required_scopes_for_rpc(&WsMethod::GitPreparePullRequestThread, Some(&payload))?;
//! assert_eq!(
//!     required,
//!     vec![EnvironmentScope::SourceControlWrite, EnvironmentScope::OrchestrationOperate]
//! );
//!
//! let partial = SessionGrantInput {
//!     authenticated: true,
//!     scopes: None,
//!     permissions: Some(vec![EnvironmentScope::SourceControlWrite]),
//!     server_update_scope: None,
//! };
//! assert!(!grants_all_scopes(&partial, &required));
//!
//! // Malformed JSON yields Err rather than panicking.
//! assert!(required_scopes_for_rpc_json(&WsMethod::ServerUpdateSettings, Some("{bad")).is_err());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod git_rules;
mod methods;
mod rules;

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use serde_json::Value;
use thiserror::Error;

pub use methods::{EmptyWsMethodError, WsMethod};
pub use rules::{contains_secret_reference, required_scopes_for_server_settings_patch};

impl FromStr for WsMethod {
    type Err = EmptyWsMethodError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl fmt::Display for WsMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for WsMethod {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for WsMethod {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(de::Error::custom)
    }
}

/// Authorization scopes governing environment RPC and HTTP access (`auth.ts:51-70`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EnvironmentScope {
    /// Read threads, runs, and projections.
    #[serde(rename = "orchestration:read")]
    OrchestrationRead,
    /// Dispatch orchestration mutations and start runs.
    #[serde(rename = "orchestration:operate")]
    OrchestrationOperate,
    /// Update server settings.
    #[serde(rename = "settings:write")]
    SettingsWrite,
    /// Manage provider instances, credentials, and secret references.
    #[serde(rename = "providers:manage")]
    ProvidersManage,
    /// Perform database maintenance and self-update operations.
    #[serde(rename = "environment:maintain")]
    EnvironmentMaintain,
    /// Control headless browser preview sessions.
    #[serde(rename = "preview:operate")]
    PreviewOperate,
    /// Read traces, process diagnostics, and telemetry history.
    #[serde(rename = "diagnostics:read")]
    DiagnosticsRead,
    /// Observe terminal streams and metadata.
    #[serde(rename = "terminal:read")]
    TerminalRead,
    /// Open, write, resize, and close terminals.
    #[serde(rename = "terminal:operate")]
    TerminalOperate,
    /// Read project files and browse directories.
    #[serde(rename = "filesystem:read")]
    FilesystemRead,
    /// Write files in project workspaces.
    #[serde(rename = "filesystem:write")]
    FilesystemWrite,
    /// Retained legacy scope; grants no current RPC access.
    #[serde(rename = "review:write")]
    ReviewWrite,
    /// Perform git, pull request, and issue tracker write actions.
    #[serde(rename = "source-control:write")]
    SourceControlWrite,
    /// List authorized client sessions and pairing grants.
    #[serde(rename = "access:read")]
    AccessRead,
    /// Mint, revoke, or approve client sessions, guest shares, and peer links.
    #[serde(rename = "access:write")]
    AccessWrite,
    /// Inspect relay connection status.
    #[serde(rename = "relay:read")]
    RelayRead,
    /// Modify relay client connections.
    #[serde(rename = "relay:write")]
    RelayWrite,
    /// Reconfigure relay enrollment and credentials.
    #[serde(rename = "relay:configure")]
    RelayConfigure,
}

impl EnvironmentScope {
    /// Returns the wire string representation of this scope.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OrchestrationRead => "orchestration:read",
            Self::OrchestrationOperate => "orchestration:operate",
            Self::SettingsWrite => "settings:write",
            Self::ProvidersManage => "providers:manage",
            Self::EnvironmentMaintain => "environment:maintain",
            Self::PreviewOperate => "preview:operate",
            Self::DiagnosticsRead => "diagnostics:read",
            Self::TerminalRead => "terminal:read",
            Self::TerminalOperate => "terminal:operate",
            Self::FilesystemRead => "filesystem:read",
            Self::FilesystemWrite => "filesystem:write",
            Self::ReviewWrite => "review:write",
            Self::SourceControlWrite => "source-control:write",
            Self::AccessRead => "access:read",
            Self::AccessWrite => "access:write",
            Self::RelayRead => "relay:read",
            Self::RelayWrite => "relay:write",
            Self::RelayConfigure => "relay:configure",
        }
    }

    /// Returns `true` if this scope is part of the original 8-scope legacy set (`auth.ts:82-91`).
    pub fn is_legacy_scope(self) -> bool {
        matches!(
            self,
            Self::OrchestrationRead
                | Self::OrchestrationOperate
                | Self::TerminalOperate
                | Self::ReviewWrite
                | Self::AccessRead
                | Self::AccessWrite
                | Self::RelayRead
                | Self::RelayWrite
        )
    }

    /// Returns the parent legacy scope used for pre-granularity server fallback (`auth.ts:103-113`).
    pub fn legacy_parent(self) -> Option<Self> {
        match self {
            Self::FilesystemRead | Self::DiagnosticsRead => Some(Self::OrchestrationRead),
            Self::SettingsWrite
            | Self::ProvidersManage
            | Self::EnvironmentMaintain
            | Self::PreviewOperate
            | Self::SourceControlWrite
            | Self::FilesystemWrite => Some(Self::OrchestrationOperate),
            Self::TerminalRead => Some(Self::TerminalOperate),
            _ => None,
        }
    }
}

/// Session authentication and grant state evaluated by [`session_grants_scope`] and [`grants_all_scopes`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionGrantInput {
    /// Whether the session is authenticated.
    pub authenticated: bool,
    /// Legacy coarse `scopes` array from session auth responses.
    pub scopes: Option<Vec<EnvironmentScope>>,
    /// Fine-grained `permissions` array from modern servers.
    pub permissions: Option<Vec<EnvironmentScope>>,
    /// Server-advertised `auth.serverUpdateScope` (when defined, legacy parent fallback is off).
    pub server_update_scope: Option<String>,
}

/// Error returned when evaluating dynamic RPC guard scopes fails (fixing BUG-17).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum GuardEvalError {
    /// Input JSON text failed to parse.
    #[error("malformed JSON payload for RPC `{method}`: {message}")]
    MalformedJson {
        /// RPC method tag whose input failed to parse.
        method: String,
        /// Underlying JSON parser error description.
        message: String,
    },
    /// Parsed JSON payload violated the schema required for scope inspection.
    #[error("invalid payload for RPC `{method}`: {reason}")]
    InvalidInput {
        /// RPC method tag whose payload failed validation.
        method: String,
        /// Validation failure description.
        reason: String,
    },
}

/// Evaluates whether `session` grants `scope` (`auth.ts:133-143`).
pub fn session_grants_scope(session: &SessionGrantInput, scope: EnvironmentScope) -> bool {
    if !session.authenticated {
        return false;
    }
    if let Some(permissions) = &session.permissions {
        return permissions.contains(&scope);
    }
    if session
        .scopes
        .as_ref()
        .is_some_and(|scopes| scopes.contains(&scope))
    {
        return true;
    }
    if session.server_update_scope.is_some() {
        return false;
    }
    scope.legacy_parent().is_some_and(|parent| {
        session
            .scopes
            .as_ref()
            .is_some_and(|scopes| scopes.contains(&parent))
    })
}

/// Returns `true` iff `session` grants **every** scope in `required` (fixing BUG-17).
pub fn grants_all_scopes(session: &SessionGrantInput, required: &[EnvironmentScope]) -> bool {
    required
        .iter()
        .all(|scope| session_grants_scope(session, *scope))
}

/// Computes all [`EnvironmentScope`]s required to execute `method` with optional parsed `input`.
///
/// Never panics or throws on malformed payloads; returns `Err(GuardEvalError::InvalidInput)`
/// when a dynamic guard receives an invalid payload structure.
pub fn required_scopes_for_rpc(
    method: &WsMethod,
    input: Option<&Value>,
) -> Result<Vec<EnvironmentScope>, GuardEvalError> {
    if let Some(payload) = input {
        match method {
            WsMethod::ServerUpdateSettings => return rules::eval_server_update_settings(payload),
            WsMethod::GitPreparePullRequestThread => {
                return git_rules::eval_git_prepare_pr_thread(payload);
            }
            WsMethod::GitRunStackedAction => {
                return git_rules::eval_git_run_stacked_action(payload);
            }
            _ => {}
        }
    }
    Ok(rules::static_guarded_scope(method).into_iter().collect())
}

/// Parses optional JSON text and evaluates [`required_scopes_for_rpc`] without panicking.
pub fn required_scopes_for_rpc_json(
    method: &WsMethod,
    input_json: Option<&str>,
) -> Result<Vec<EnvironmentScope>, GuardEvalError> {
    match input_json {
        None => required_scopes_for_rpc(method, None),
        Some(raw) => {
            let parsed: Value =
                serde_json::from_str(raw).map_err(|err| GuardEvalError::MalformedJson {
                    method: method.as_str().to_owned(),
                    message: err.to_string(),
                })?;
            required_scopes_for_rpc(method, Some(&parsed))
        }
    }
}

/// Computes scopes required for a project mutation (`settings.ts:2037-2047`).
pub fn required_scopes_for_project_mutation(
    mutation_type: &str,
    scripts: Option<&Value>,
) -> Result<Vec<EnvironmentScope>, GuardEvalError> {
    if mutation_type == "project.delete" || scripts.is_none() {
        return Ok(vec![EnvironmentScope::OrchestrationOperate]);
    }
    let mut patch = serde_json::Map::new();
    if let Some(scripts_val) = scripts {
        patch.insert("defaultProjectScripts".to_owned(), scripts_val.clone());
    }
    let mut scopes = vec![EnvironmentScope::OrchestrationOperate];
    scopes.extend(required_scopes_for_server_settings_patch(&patch)?);
    Ok(scopes)
}
