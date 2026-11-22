//! Checkpoint diff persistence, `similar`-based unified diff generation, and `BUG-02` rollback guards.
//!
//! Operates on `legion-persistence`'s `checkpoint_diff_blobs` table, hidden git
//! checkpoint refs (`refs/legioncode/checkpoints/<thread_id>/turn-<n>`), whitespace-preserving
//! unified diff computation, and SQLite pending-intent verification before git restore.

pub mod diff_patch_fast;
pub mod diff_query;
pub mod rollback;
pub mod store;

use legion_contracts::ids::{CheckpointRef, ThreadId};
use thiserror::Error;

/// Prefix under which all `LegionCode` hidden checkpoint refs are stored.
pub const CHECKPOINT_REF_PREFIX: &str = "refs/legioncode/checkpoints";

/// Errors returned by checkpoint storage, diff query, and rollback planning.
#[derive(Debug, Error)]
pub enum CheckpointError {
    /// Underlying `SQLite` query or transaction failure.
    #[error("SQLite checkpoint error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// Malformed or unsafe git checkpoint reference or thread identifier.
    #[error("invalid git checkpoint reference `{value}`: {reason}")]
    InvalidRef {
        /// Original reference or identifier string.
        value: String,
        /// Validation failure explanation.
        reason: String,
    },
    /// Attempted rollback restore without a verified durable `rollback.pending` intent (`BUG-02`).
    #[error("rollback restore guard rejected request: {0}")]
    RollbackGuard(String),
    /// Invalid input parameter or range mismatch.
    #[error("invalid checkpoint input: {0}")]
    InvalidInput(String),
}

/// Validates that `component` is a safe single path segment for a git reference.
///
/// Rejects empty strings, leading/trailing dots, `.lock` suffixes, whitespace,
/// control characters, slashes, `..`, `@{`, `:`, `~`, `^`, `?`, `*`, `[`, and `\`.
///
/// # Errors
///
/// Returns [`CheckpointError::InvalidRef`] if `component` violates git ref rules.
pub fn validate_git_ref_component(component: &str) -> Result<(), CheckpointError> {
    if component.is_empty()
        || component.starts_with('.')
        || component.ends_with('.')
        || std::path::Path::new(component)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("lock"))
        || component.starts_with('-')
        || component.contains("..")
        || component.contains("@{")
    {
        return Err(CheckpointError::InvalidRef {
            value: component.to_owned(),
            reason: "contains forbidden git ref sequence or boundary character".to_owned(),
        });
    }

    for ch in component.chars() {
        if ch.is_control()
            || ch.is_whitespace()
            || matches!(ch, '/' | '\\' | ':' | '~' | '^' | '?' | '*' | '[')
        {
            return Err(CheckpointError::InvalidRef {
                value: component.to_owned(),
                reason: format!("contains forbidden git ref character `{ch}`"),
            });
        }
    }
    Ok(())
}

/// Builds the hidden git checkpoint ref `refs/legioncode/checkpoints/<thread_id>/turn-<n>`.
///
/// # Errors
///
/// Returns [`CheckpointError::InvalidRef`] when `thread_id` cannot form a safe git ref
/// component or violates [`CheckpointRef`] contract rules.
///
/// ```
/// use legion_contracts::ids::ThreadId;
/// use legion_scm::checkpoints::checkpoint_ref_for_turn;
///
/// let thread_id = ThreadId::new("thread-abc123")?;
/// let git_ref = checkpoint_ref_for_turn(&thread_id, 4)?;
/// assert_eq!(git_ref.as_str(), "refs/legioncode/checkpoints/thread-abc123/turn-4");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn checkpoint_ref_for_turn(
    thread_id: &ThreadId,
    turn_count: u32,
) -> Result<CheckpointRef, CheckpointError> {
    validate_git_ref_component(thread_id.as_str())?;
    let formatted = format!(
        "{CHECKPOINT_REF_PREFIX}/{}/turn-{turn_count}",
        thread_id.as_str()
    );
    CheckpointRef::new(formatted).map_err(|err| CheckpointError::InvalidRef {
        value: thread_id.as_str().to_owned(),
        reason: err.to_string(),
    })
}

/// Parses `refs/legioncode/checkpoints/<thread_id>/turn-<n>` into `(thread_id, turn_count)`.
#[must_use]
pub fn parse_checkpoint_ref(raw_ref: &str) -> Option<(String, u32)> {
    let tail = raw_ref
        .strip_prefix(CHECKPOINT_REF_PREFIX)?
        .strip_prefix('/')?;
    let (thread_part, turn_part) = tail.split_once('/')?;
    if thread_part.is_empty() || turn_part.contains('/') {
        return None;
    }
    let turn_str = turn_part.strip_prefix("turn-")?;
    if turn_str.is_empty() {
        return None;
    }
    let turn = turn_str.parse::<u32>().ok()?;
    validate_git_ref_component(thread_part).ok()?;
    Some((thread_part.to_owned(), turn))
}
