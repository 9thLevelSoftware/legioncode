//! Strongly typed, serde-validated entity identifier newtypes.
//!
//! Mirrors `packages/contracts/src/baseSchemas.ts`, `providerInstance.ts`,
//! `issue.ts`, and `sourceControl.ts`: entity IDs trim leading/trailing
//! whitespace on construction and deserialization and reject blank values, while
//! slug identifiers also enforce length and character constraints.
//!
//! # Examples
//!
//! ```
//! use t3_contracts::ids::{EnvironmentId, ProviderInstanceId, ThreadId};
//!
//! let env_id = EnvironmentId::new("  env-local  ")?;
//! assert_eq!(env_id.as_str(), "env-local");
//!
//! let thread_id: ThreadId = serde_json::from_str("\"thread-1\"")?;
//! assert_eq!(thread_id.to_string(), "thread-1");
//!
//! assert!(ThreadId::new("   ").is_err());
//! assert!(ProviderInstanceId::new("1invalid").is_err());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use thiserror::Error;

const SLUG_MAX_CHARS: usize = 64;

/// Validation error returned when constructing a typed identifier fails.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum IdError {
    /// The input was empty or contained only whitespace.
    #[error("{kind} cannot be empty or whitespace-only")]
    Empty {
        /// Human-readable identifier type name.
        kind: &'static str,
    },
    /// The input exceeded the maximum allowed character length.
    #[error("{kind} exceeds maximum length of {max} characters")]
    TooLong {
        /// Human-readable identifier type name.
        kind: &'static str,
        /// Maximum character count permitted for this identifier.
        max: usize,
    },
    /// The input did not satisfy the required character format.
    #[error("{kind} has invalid format: {expected}")]
    InvalidFormat {
        /// Human-readable identifier type name.
        kind: &'static str,
        /// Description of the expected character pattern.
        expected: &'static str,
    },
}

fn trim_non_empty(kind: &'static str, raw: &str) -> Result<String, IdError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(IdError::Empty { kind });
    }
    Ok(trimmed.to_owned())
}

fn validate_provider_slug(kind: &'static str, raw: &str) -> Result<String, IdError> {
    let trimmed = trim_non_empty(kind, raw)?;
    if trimmed.chars().count() > SLUG_MAX_CHARS {
        return Err(IdError::TooLong {
            kind,
            max: SLUG_MAX_CHARS,
        });
    }
    let mut chars = trimmed.chars();
    let first_ok = chars.next().is_some_and(|ch| ch.is_ascii_alphabetic());
    let rest_ok = chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-');
    if !(first_ok && rest_ok) {
        return Err(IdError::InvalidFormat {
            kind,
            expected: "ASCII letter followed by ASCII letters, digits, '_', or '-'",
        });
    }
    Ok(trimmed)
}

fn validate_lowercase_kind(kind: &'static str, raw: &str) -> Result<String, IdError> {
    let trimmed = trim_non_empty(kind, raw)?;
    if trimmed.chars().count() > SLUG_MAX_CHARS {
        return Err(IdError::TooLong {
            kind,
            max: SLUG_MAX_CHARS,
        });
    }
    let mut chars = trimmed.chars();
    let first_ok = chars.next().is_some_and(|ch| ch.is_ascii_lowercase());
    let rest_ok = chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-');
    if !(first_ok && rest_ok) {
        return Err(IdError::InvalidFormat {
            kind,
            expected: "lowercase ASCII letter followed by lowercase ASCII letters, digits, or '-'",
        });
    }
    Ok(trimmed)
}

macro_rules! define_id_type {
    ($(#[$meta:meta])* $name:ident, $validator:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(String);

        impl $name {
            /// Parses and validates a raw string, trimming surrounding whitespace.
            pub fn new(raw: impl AsRef<str>) -> Result<Self, IdError> {
                $validator(stringify!($name), raw.as_ref()).map(Self)
            }

            /// Returns the validated, trimmed identifier string slice.
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Consumes the identifier and returns the underlying string.
            pub fn into_string(self) -> String {
                self.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl TryFrom<&str> for $name {
            type Error = IdError;

            fn try_from(value: &str) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl TryFrom<String> for $name {
            type Error = IdError;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl FromStr for $name {
            type Err = IdError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Self::new(s)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let raw = String::deserialize(deserializer)?;
                Self::new(raw).map_err(de::Error::custom)
            }
        }
    };
}

define_id_type!(
    /// Unique identifier for an execution environment (server instance).
    EnvironmentId,
    trim_non_empty
);
define_id_type!(
    /// Unique identifier for a workspace project rooted at a directory.
    ProjectId,
    trim_non_empty
);
define_id_type!(
    /// Unique identifier for a durable orchestration thread.
    ThreadId,
    trim_non_empty
);
define_id_type!(
    /// Unique identifier for an orchestration turn run.
    RunId,
    trim_non_empty
);
define_id_type!(
    /// Unique idempotency identifier for an orchestration command.
    CommandId,
    trim_non_empty
);
define_id_type!(
    /// Unique identifier for a conversation message.
    MessageId,
    trim_non_empty
);
define_id_type!(
    /// Unique identifier for a turn timeline item.
    TurnItemId,
    trim_non_empty
);
define_id_type!(
    /// Unique identifier for a persisted orchestration domain event.
    EventId,
    trim_non_empty
);
define_id_type!(
    /// Unique identifier for an execution node or subagent.
    NodeId,
    trim_non_empty
);
define_id_type!(
    /// Unique identifier for a run execution attempt.
    RunAttemptId,
    trim_non_empty
);
define_id_type!(
    /// Unique identifier for a guest thread share link.
    GuestShareId,
    trim_non_empty
);
define_id_type!(
    /// Unique identifier for a guest-submitted message.
    GuestMessageId,
    trim_non_empty
);
define_id_type!(
    /// Unique identifier for a live provider session.
    ProviderSessionId,
    trim_non_empty
);
define_id_type!(
    /// Unique identifier for a provider-native thread.
    ProviderThreadId,
    trim_non_empty
);
define_id_type!(
    /// Unique identifier for a provider-native turn.
    ProviderTurnId,
    trim_non_empty
);
define_id_type!(
    /// Unique identifier for an interactive approval or user-input request.
    RuntimeRequestId,
    trim_non_empty
);
define_id_type!(
    /// Unique identifier for a scheduled task.
    ScheduledTaskId,
    trim_non_empty
);
define_id_type!(
    /// Unique identifier for a filesystem checkpoint.
    CheckpointId,
    trim_non_empty
);
define_id_type!(
    /// Hidden git ref naming a captured checkpoint.
    CheckpointRef,
    trim_non_empty
);
define_id_type!(
    /// Unique identifier for a nested checkpoint scope.
    CheckpointScopeId,
    trim_non_empty
);
define_id_type!(
    /// Unique identifier for a portable context handoff artifact.
    ContextHandoffId,
    trim_non_empty
);
define_id_type!(
    /// Unique identifier for a cross-provider context transfer.
    ContextTransferId,
    trim_non_empty
);
define_id_type!(
    /// Raw provider protocol event identifier.
    RawEventId,
    trim_non_empty
);
define_id_type!(
    /// Unique identifier for a plan or todo-list artifact.
    PlanId,
    trim_non_empty
);
define_id_type!(
    /// Slug identifying a configured provider instance (`^[a-zA-Z][a-zA-Z0-9_-]*$`, max 64 chars).
    ProviderInstanceId,
    validate_provider_slug
);
define_id_type!(
    /// Slug identifying a provider driver kind (`^[a-zA-Z][a-zA-Z0-9_-]*$`, max 64 chars).
    ProviderDriverKind,
    validate_provider_slug
);
define_id_type!(
    /// Slug identifying a source control host kind (`^[a-z][a-z0-9-]*$`, max 64 chars).
    SourceControlProviderKind,
    validate_lowercase_kind
);
define_id_type!(
    /// Slug identifying an issue tracker kind (`^[a-z][a-z0-9-]*$`, max 64 chars).
    IssueTrackerKind,
    validate_lowercase_kind
);
