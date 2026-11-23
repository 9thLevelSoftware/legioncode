//! `legioncode thread` CLI error mapping and formatting helpers (`cli/thread.ts`).

use legion_contracts::brand::ACTIVE_BRAND;
use thiserror::Error;

use super::types::{ThreadListItem, ThreadSendDelivery};

/// Bounded wait slice matching `WAIT_SLICE_MS` in `apps/server/src/cli/thread.ts:42`.
pub const THREAD_WAIT_SLICE_MS: u64 = 15_000;
/// Default page limit for `legion_thread_read` timeline paging (`thread.ts:52`).
pub const THREAD_READ_PAGE_LIMIT: u32 = 100;
/// Maximum characters per timeline item returned in a single page (`thread.ts:53`).
pub const THREAD_READ_ITEM_MAX_CHARS: u32 = 20_000;
/// Maximum derived title length from a message's first line (`thread.ts:54`).
pub const THREAD_TITLE_MAX_CHARS: usize = 80;

/// Typed errors mirroring `cli/thread.ts:56-138` including exit code `2` on [`Self::NeedsInput`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ThreadClientError {
    /// No running server found in `<base_dir>/userdata` or `<base_dir>/dev` (`exit_code = 1`).
    #[error(
        "No running {} server uses {base_dir}. Start {}, then retry.",
        ACTIVE_BRAND.product_name,
        ACTIVE_BRAND.product_name
    )]
    ServerNotRunning {
        /// Base home directory checked for a live server.
        base_dir: String,
    },
    /// Transport/HTTP error talking to the server's `/mcp` endpoint (`exit_code = 1`).
    #[error(
        "Could not reach the {} server at {origin}.{detail}",
        ACTIVE_BRAND.product_name
    )]
    ServerUnreachable {
        /// Server origin URL.
        origin: String,
        /// Leading-space detail string (`""` or `" <err>"`).
        detail: String,
    },
    /// MCP tool returned `isError: true` or an invalid payload (`exit_code = 1`).
    #[error("{detail}")]
    ToolFailed {
        /// MCP tool name (e.g. `"legion_thread_list"`).
        tool: String,
        /// Server or schema diagnostic text.
        detail: String,
    },
    /// CLI usage/argument validation error (`exit_code = 1`).
    #[error("{detail}")]
    Usage {
        /// Validation message.
        detail: String,
    },
    /// Requested project identifier or path did not match any project (`exit_code = 1`).
    #[error(
        "No {} project matches '{identifier}'. Pass a project id or a path inside a project.",
        ACTIVE_BRAND.product_name
    )]
    ProjectNotFound {
        /// Unmatched identifier or filesystem path.
        identifier: String,
    },
    /// Active run paused awaiting human input (`ThreadNeedsInputError`, `exit_code = 2`).
    #[error(
        "Thread {thread_id} is waiting for an answer. Open it in {} to respond.",
        ACTIVE_BRAND.product_name
    )]
    NeedsInput {
        /// Blocked thread ID.
        thread_id: String,
    },
    /// Run terminated in a non-completed terminal state (`exit_code = 1`).
    #[error("Thread {thread_id} stopped with status {status}{}", error.as_deref().map_or_else(|| ".".to_owned(), |e| format!(": {e}")))]
    RunEnded {
        /// Thread identifier.
        thread_id: String,
        /// Terminal status string.
        status: String,
        /// Optional failure reason.
        error: Option<String>,
    },
}

impl ThreadClientError {
    /// Returns process exit code (`2` for [`Self::NeedsInput`], `1` for all other errors).
    #[must_use]
    pub const fn exit_code(&self) -> i32 {
        match self {
            Self::NeedsInput { .. } => 2,
            _ => 1,
        }
    }
}

/// Formats one line matching `legioncode thread list` (`thread.ts:810`).
#[must_use]
pub fn format_thread_list_line(item: &ThreadListItem) -> String {
    format!("{}  {:<11}  {}", item.thread_id, item.status, item.title)
}

/// Formats `legioncode thread send` confirmation (`"Queued."` vs `"Sent."`, `thread.ts:928`).
#[must_use]
pub const fn format_thread_send_ack(delivery: ThreadSendDelivery) -> &'static str {
    match delivery {
        ThreadSendDelivery::Queued => "Queued.",
        _ => "Sent.",
    }
}

/// Derives a bounded title from the first trimmed line of `message` (`thread.ts:691-701`).
#[must_use]
pub fn format_title_from_message(message: &str) -> String {
    let first_line = message.trim().lines().next().map_or("", str::trim);
    let mut utf16_len = 0usize;
    let mut end_byte = 0usize;
    for (idx, ch) in first_line.char_indices() {
        let next_u16 = utf16_len + ch.len_utf16();
        if next_u16 > THREAD_TITLE_MAX_CHARS {
            return format!("{}…", first_line[..end_byte].trim_end());
        }
        if next_u16 < THREAD_TITLE_MAX_CHARS {
            end_byte = idx + ch.len_utf8();
        }
        utf16_len = next_u16;
    }
    first_line.to_owned()
}
