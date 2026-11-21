//! Secret-request answer and failure contracts (`secretRequest.ts`).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ids::{ThreadId, TurnItemId};

/// Failure reasons returned when validating or consuming a secret request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretRequestFailureReason {
    LoadFailed,
    NotFound,
    AlreadyAnswered,
    /// Multi-value field keys in the client's answer do not match the request's declared fields.
    FieldsMismatch,
    AgentStopped,
    StoreFailed,
    ReferenceUnreadable,
    RecordFailed,
    InvalidRef,
    ReadFailed,
    RefUnavailable,
    RefExpired,
    ConsumeFailed,
    NotAFileRequest,
}

/// Answer variants accepted by `secrets.answerRequest` (`secretRequest.ts`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SecretRequestAnswer {
    /// Save either a single secret value or multiple named field values (`0bc9b57b93`).
    Save {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        secret: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        values: Option<BTreeMap<String, String>>,
    },
    /// Decline to supply the requested secret(s).
    Decline,
    /// Resolve a reference via a configured external secret provider.
    Reference { reference: serde_json::Value },
}

/// Payload for `secrets.answerRequest`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretRequestAnswerInput {
    pub thread_id: ThreadId,
    pub turn_item_id: TurnItemId,
    pub answer: SecretRequestAnswer,
}

/// Contract error for `secrets.answerRequest`.
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[error("Secret request failed: {reason:?}")]
pub struct SecretRequestError {
    pub reason: SecretRequestFailureReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}
