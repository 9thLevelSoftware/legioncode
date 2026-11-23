//! Unified error type for `legion-server-runtime`.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use thiserror::Error;

/// Top-level error returned by `legion-server-runtime` components.
#[derive(Debug, Error)]
pub enum ServerRuntimeError {
    /// Invalid caller input or malformed request payload.
    #[error("bad request: {0}")]
    BadRequest(String),
    /// Missing or rejected authentication credential.
    #[error("unauthorized: {0}")]
    Unauthorized(String),
    /// Authenticated caller lacks required authorization scopes (`BUG-04`).
    #[error("forbidden: {0}")]
    Forbidden(String),
    /// Requested entity or file does not exist.
    #[error("not found: {0}")]
    NotFound(String),
    /// State conflict such as closed terminal resume, duplicate project, or missing git repo.
    #[error("conflict: {0}")]
    Conflict(String),
    /// Git subprocess invocation failed.
    #[error("git command `{command}` failed: {message}")]
    Git {
        /// Subcommand executed.
        command: String,
        /// Error detail captured from stderr or status.
        message: String,
    },
    /// Database, schema, or orchestration persistence failure.
    #[error("persistence error: {0}")]
    Persistence(String),
    /// Subprocess spawn, stdio transport, CDP, or provider protocol error.
    #[error("process transport error: {0}")]
    Process(String),
    /// Upstream HTTP/forge/peer API failure.
    #[error("upstream integration error: {0}")]
    Upstream(String),
    /// Terminal runtime or VT decoding error.
    #[error("terminal runtime error: {0}")]
    Terminal(String),
    /// Filesystem I/O failure.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

impl From<legion_scm::checkpoints::CheckpointError> for ServerRuntimeError {
    fn from(err: legion_scm::checkpoints::CheckpointError) -> Self {
        Self::Conflict(err.to_string())
    }
}

impl From<legion_scm::live_http::LiveClientError> for ServerRuntimeError {
    fn from(err: legion_scm::live_http::LiveClientError) -> Self {
        match err {
            legion_scm::live_http::LiveClientError::InvalidInput(msg) => Self::BadRequest(msg),
            other => Self::Upstream(other.to_string()),
        }
    }
}

impl From<legion_persistence::pairing_tokens::PairingTokenError> for ServerRuntimeError {
    fn from(err: legion_persistence::pairing_tokens::PairingTokenError) -> Self {
        use legion_persistence::pairing_tokens::PairingTokenError;
        match err {
            PairingTokenError::UnknownToken
            | PairingTokenError::AlreadyConsumed
            | PairingTokenError::TokenRevoked
            | PairingTokenError::TokenExpired { .. }
            | PairingTokenError::SessionRevoked { .. }
            | PairingTokenError::SessionExpired { .. } => Self::Unauthorized(err.to_string()),
            PairingTokenError::InvalidTtl { .. } => Self::BadRequest(err.to_string()),
            PairingTokenError::OpenDatabase { .. }
            | PairingTokenError::Sqlite { .. }
            | PairingTokenError::Json { .. } => Self::Persistence(err.to_string()),
        }
    }
}

impl From<legion_persistence::auth_records::AuthRecordsError> for ServerRuntimeError {
    fn from(err: legion_persistence::auth_records::AuthRecordsError) -> Self {
        Self::Persistence(err.to_string())
    }
}

impl From<legion_persistence::project_store::ProjectStoreError> for ServerRuntimeError {
    fn from(err: legion_persistence::project_store::ProjectStoreError) -> Self {
        use legion_persistence::project_store::ProjectStoreError;
        match err {
            ProjectStoreError::InvalidTitle
            | ProjectStoreError::InvalidWorkspaceRoot
            | ProjectStoreError::InvalidScriptsJson => Self::BadRequest(err.to_string()),
            ProjectStoreError::NotFound { target } => Self::NotFound(target),
            ProjectStoreError::WorkspaceConflict { .. }
            | ProjectStoreError::ProjectConflict { .. }
            | ProjectStoreError::ProjectNotEmpty { .. } => Self::Conflict(err.to_string()),
            ProjectStoreError::OpenDatabase { .. } | ProjectStoreError::Sqlite { .. } => {
                Self::Persistence(err.to_string())
            }
        }
    }
}

impl From<legion_remote::peer::links::PeerLinkError> for ServerRuntimeError {
    fn from(err: legion_remote::peer::links::PeerLinkError) -> Self {
        use legion_remote::peer::links::PeerLinkErrorReason;
        match err.reason {
            PeerLinkErrorReason::UnknownLink => Self::NotFound(err.message),
            PeerLinkErrorReason::SelfLink => Self::BadRequest(err.message),
            PeerLinkErrorReason::PairingRejected | PeerLinkErrorReason::Expired => {
                Self::Unauthorized(err.message)
            }
            PeerLinkErrorReason::Storage => Self::Persistence(err.message),
            _ => Self::Upstream(err.message),
        }
    }
}

impl From<legion_remote::live_network::live_peer_mcp::PeerMcpError> for ServerRuntimeError {
    fn from(err: legion_remote::live_network::live_peer_mcp::PeerMcpError) -> Self {
        Self::Upstream(err.to_string())
    }
}

impl From<legion_orchestrator::OrchestratorError> for ServerRuntimeError {
    fn from(err: legion_orchestrator::OrchestratorError) -> Self {
        Self::Persistence(err.to_string())
    }
}

impl From<legion_orchestrator::sink::SinkError> for ServerRuntimeError {
    fn from(err: legion_orchestrator::sink::SinkError) -> Self {
        Self::Persistence(err.to_string())
    }
}

impl From<legion_terminal_vt::VtError> for ServerRuntimeError {
    fn from(err: legion_terminal_vt::VtError) -> Self {
        Self::Terminal(err.to_string())
    }
}

impl From<rusqlite::Error> for ServerRuntimeError {
    fn from(err: rusqlite::Error) -> Self {
        Self::Persistence(err.to_string())
    }
}

impl From<serde_json::Error> for ServerRuntimeError {
    fn from(err: serde_json::Error) -> Self {
        Self::BadRequest(err.to_string())
    }
}

impl IntoResponse for ServerRuntimeError {
    fn into_response(self) -> Response {
        let status = match &self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            Self::Forbidden(_) => StatusCode::FORBIDDEN,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::Upstream(_) => StatusCode::BAD_GATEWAY,
            Self::Git { .. }
            | Self::Persistence(_)
            | Self::Process(_)
            | Self::Terminal(_)
            | Self::Io(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(json!({ "error": self.to_string() }))).into_response()
    }
}
