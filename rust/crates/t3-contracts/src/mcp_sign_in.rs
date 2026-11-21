//! Provider MCP server OAuth sign-in contracts (`provider.ts` & `orchestrationV2.ts`).

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ids::{ProviderThreadId, ThreadId};

/// Status of a provider MCP server sign-in flow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum McpSignInStatus {
    /// Browser sign-in URL issued; waiting for the user's OAuth callback URL.
    #[serde(rename_all = "camelCase")]
    Pending {
        /// MCP server name requiring authentication.
        server: String,
        /// Authorization page URL to open in a browser.
        authorization_url: String,
    },
    /// Server is already or newly authenticated on the environment.
    #[serde(rename_all = "camelCase")]
    SignedIn {
        /// MCP server name.
        server: String,
        /// Environment ISO-8601 timestamp when sign-in completed.
        signed_in_at: String,
    },
}

/// Notice metadata attached to a `system_notice` turn item offering inline MCP sign-in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestrationMcpSignInNotice {
    /// Configured MCP server identifier that requested sign-in.
    pub server: String,
}

/// Input payload for `provider.mcpSignIn.start`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderMcpSignInStartInput {
    /// Owning thread ID.
    pub thread_id: ThreadId,
    /// Native/provider thread ID whose session owns the MCP server.
    pub provider_thread_id: ProviderThreadId,
    /// Target MCP server name.
    pub server: String,
}

/// Output payload for `provider.mcpSignIn.start`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderMcpSignInStartResult {
    /// Browser authorization URL (`None` when already signed in).
    pub authorization_url: Option<String>,
    /// Environment timestamp when already signed in (`None` when authorization is required).
    pub signed_in_at: Option<String>,
}

/// Input payload for `provider.mcpSignIn.complete`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderMcpSignInCompleteInput {
    /// Owning thread ID.
    pub thread_id: ThreadId,
    /// Provider thread ID whose session owns the MCP server.
    pub provider_thread_id: ProviderThreadId,
    /// Target MCP server name.
    pub server: String,
    /// Redirect callback URL copied/forwarded after browser approval.
    pub callback_url: String,
}

/// Output payload for `provider.mcpSignIn.complete`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderMcpSignInCompleteResult {
    /// Environment ISO-8601 timestamp when sign-in finished.
    pub signed_in_at: String,
}

/// Tagged sign-in request union (`provider.mcpSignIn.{start,complete}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "lowercase")]
pub enum McpSignInRequest {
    /// Initiates an OAuth flow for an MCP server in a session.
    Start(ProviderMcpSignInStartInput),
    /// Completes the OAuth flow with the redirect callback URL.
    Complete(ProviderMcpSignInCompleteInput),
}

/// Safe user-facing error from MCP OAuth sign-in (never echoes callback URLs or tokens).
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[error("{detail}")]
pub struct ProviderMcpSignInError {
    /// MCP server name associated with the failure.
    pub server: String,
    /// Redacted failure message safe for display.
    pub detail: String,
}
