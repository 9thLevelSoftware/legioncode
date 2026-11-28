//! Error codes and tagged RPC error payload for Project Workflow Board.

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use thiserror::Error;

use super::validators::{deserialize_opt_positive_revision, is_valid_artifact_key};

/// Machine-readable error codes emitted in [`WorkflowBoardRequestError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowBoardRequestErrorCode {
    /// Requested card does not exist (or belongs to another project).
    CardNotFound,
    /// Target project does not exist.
    ProjectNotFound,
    /// Optimistic concurrency check failed (`expectedRevision != currentRevision`).
    RevisionConflict,
    /// Requested stage transition is invalid.
    InvalidStageTransition,
    /// Stage launch or plan approval requires an approved spec.
    SpecNotApproved,
    /// Execution requires an approved plan.
    PlanNotApproved,
    /// Exploration decisions remain unresolved.
    ExplorationUnresolved,
    /// Challenge concerns remain unresolved.
    ChallengeUnresolved,
    /// Unreconciled external disk edit detected.
    ExternalEditDetected,
    /// Task file contract or DAG conflict.
    ContractConflict,
    /// Expected artifact digest did not match persisted digest.
    DigestMismatch,
    /// Artifact key failed relative-path validation.
    InvalidArtifactKey,
    /// Requested artifact does not exist.
    ArtifactNotFound,
    /// Caller supplied invalid input.
    InvalidInput,
    /// Thread or stage launch failed.
    LaunchFailed,
    /// Unexpected internal persistence or runtime error.
    InternalError,
}

/// Tagged error payload returned by `workflowBoard.*` RPC handlers (`_tag: "WorkflowBoardRequestError"`).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{message}")]
pub struct WorkflowBoardRequestError {
    /// Machine-readable error code.
    pub code: WorkflowBoardRequestErrorCode,
    /// Human-readable error message.
    pub message: String,
    /// Authoritative current revision when `code == RevisionConflict`.
    pub current_revision: Option<u64>,
}

impl WorkflowBoardRequestError {
    /// Creates a new [`WorkflowBoardRequestError`] without a revision hint.
    #[must_use]
    pub fn new(code: WorkflowBoardRequestErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            current_revision: None,
        }
    }

    /// Attaches the current card revision (`>= 1`) for conflict diagnostics.
    #[must_use]
    pub fn with_current_revision(mut self, revision: u64) -> Self {
        self.current_revision = Some(revision);
        self
    }

    /// Returns `true` when `key` satisfies relative artifact key constraints.
    #[must_use]
    pub fn is_valid_artifact_key(key: &str) -> bool {
        is_valid_artifact_key(key)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireWorkflowBoardRequestError<'a> {
    #[serde(rename = "_tag")]
    tag: &'a str,
    code: WorkflowBoardRequestErrorCode,
    message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    current_revision: Option<u64>,
}

impl Serialize for WorkflowBoardRequestError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let wire = WireWorkflowBoardRequestError {
            tag: "WorkflowBoardRequestError",
            code: self.code,
            message: self.message.clone(),
            current_revision: self.current_revision,
        };
        wire.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for WorkflowBoardRequestError {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct OwnedWire {
            #[serde(rename = "_tag")]
            tag: String,
            code: WorkflowBoardRequestErrorCode,
            message: String,
            #[serde(default, deserialize_with = "deserialize_opt_positive_revision")]
            current_revision: Option<u64>,
        }
        let wire = OwnedWire::deserialize(deserializer)?;
        if wire.tag != "WorkflowBoardRequestError" {
            return Err(de::Error::custom(format!(
                "expected _tag \"WorkflowBoardRequestError\", got \"{}\"",
                wire.tag
            )));
        }
        Ok(Self {
            code: wire.code,
            message: wire.message,
            current_revision: wire.current_revision,
        })
    }
}
