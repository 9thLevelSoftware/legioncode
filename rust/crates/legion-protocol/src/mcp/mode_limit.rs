//! `x-legioncode-mode-limit` header parsing, permission ceiling intersection, and `LinkOrigin` fencing.
//!
//! Mirrors `apps/server/src/mcp/McpInvocationContext.ts` and
//! `apps/server/src/mcp/linkOrigin.ts`.

use legion_contracts::orchestration::{LinkOrigin, McpClientAccess, ProviderInteractionMode};
use legion_contracts::policy::RuntimeMode;
use thiserror::Error;

/// Request header carrying peer-supplied mode ceiling (`<runtimeMode>/<interactionMode>`).
pub const HEADER_MODE_LIMIT: &str = "x-legioncode-mode-limit";

/// Request header identifying the sending peer thread inside a link chain.
pub const HEADER_SENDER_THREAD: &str = "x-legioncode-sender-thread";

/// Effective `(runtime_mode, interaction_mode)` upper bound enforced for an MCP session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModeCeiling {
    /// Broadest runtime permission tier permitted.
    pub runtime_mode: RuntimeMode,
    /// Broadest provider interaction mode permitted (`Plan` restricts `Default`).
    pub interaction_mode: ProviderInteractionMode,
}

/// Errors raised when parsing `x-legioncode-mode-limit` or enforcing `LinkOrigin` boundaries.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum McpAccessError {
    /// `x-legioncode-mode-limit` header value did not match `<runtimeMode>/<interactionMode>`.
    #[error("invalid {HEADER_MODE_LIMIT} header {raw:?}")]
    InvalidModeLimit {
        /// Malformed raw header value.
        raw: String,
    },
    /// Linked MCP caller tried to read or mutate a thread outside its `LinkOrigin` session.
    #[error("linked MCP session {session_id:?} cannot access thread outside its own link")]
    OutsideLink {
        /// Caller's `LinkOrigin.session_id`.
        session_id: String,
    },
    /// Linked MCP caller invoked a host-only operation blocked by `assert_not_linked`.
    #[error("operation {operation:?} is forbidden for linked MCP session {session_id:?}")]
    OperationForbiddenForLinkedClient {
        /// Caller's `LinkOrigin.session_id`.
        session_id: String,
        /// Blocked host-only operation name.
        operation: String,
    },
}

/// Numeric privilege rank for [`ProviderInteractionMode`] (`plan = 0`, `default = 1`).
pub fn interaction_mode_rank(mode: ProviderInteractionMode) -> u8 {
    match mode {
        ProviderInteractionMode::Plan => 0,
        ProviderInteractionMode::Default => 1,
    }
}

/// Parses an `x-legioncode-mode-limit` header value in `<runtimeMode>/<interactionMode>` form.
pub fn parse_mode_limit(raw: &str) -> Result<ModeCeiling, McpAccessError> {
    let trimmed = raw.trim();
    let Some((runtime_part, interaction_part)) = trimmed.split_once('/') else {
        return Err(McpAccessError::InvalidModeLimit {
            raw: raw.to_owned(),
        });
    };
    if interaction_part.contains('/') {
        return Err(McpAccessError::InvalidModeLimit {
            raw: raw.to_owned(),
        });
    }

    let runtime_mode = match runtime_part.trim() {
        "approval-required" => RuntimeMode::ApprovalRequired,
        "auto-accept-edits" => RuntimeMode::AutoAcceptEdits,
        "auto" => RuntimeMode::Auto,
        "full-access" => RuntimeMode::FullAccess,
        _ => {
            return Err(McpAccessError::InvalidModeLimit {
                raw: raw.to_owned(),
            });
        }
    };

    let interaction_mode = match interaction_part.trim() {
        "default" => ProviderInteractionMode::Default,
        "plan" => ProviderInteractionMode::Plan,
        _ => {
            return Err(McpAccessError::InvalidModeLimit {
                raw: raw.to_owned(),
            });
        }
    };

    Ok(ModeCeiling {
        runtime_mode,
        interaction_mode,
    })
}

/// Formats a [`ModeCeiling`] back into canonical `x-legioncode-mode-limit` header representation.
pub fn format_mode_limit(ceiling: ModeCeiling) -> String {
    let runtime = match ceiling.runtime_mode {
        RuntimeMode::ApprovalRequired => "approval-required",
        RuntimeMode::AutoAcceptEdits => "auto-accept-edits",
        RuntimeMode::Auto => "auto",
        RuntimeMode::FullAccess => "full-access",
    };
    let interaction = match ceiling.interaction_mode {
        ProviderInteractionMode::Default => "default",
        ProviderInteractionMode::Plan => "plan",
    };
    format!("{runtime}/{interaction}")
}

/// Derives the baseline [`ModeCeiling`] granted by an [`McpClientAccess`] approval tier.
pub fn approved_mode_ceiling(access: Option<McpClientAccess>) -> ModeCeiling {
    let runtime_mode = match access {
        None | Some(McpClientAccess::ReadOnly | McpClientAccess::ApprovalRequired) => {
            RuntimeMode::ApprovalRequired
        }
        Some(McpClientAccess::AutoAcceptEdits) => RuntimeMode::AutoAcceptEdits,
        Some(McpClientAccess::Auto) => RuntimeMode::Auto,
        Some(McpClientAccess::FullAccess) => RuntimeMode::FullAccess,
    };
    ModeCeiling {
        runtime_mode,
        interaction_mode: ProviderInteractionMode::Default,
    }
}

/// Returns `true` when `access` only permits read-only MCP queries (`None` or `ReadOnly`).
pub fn is_read_only_access(access: Option<McpClientAccess>) -> bool {
    matches!(access, None | Some(McpClientAccess::ReadOnly))
}

fn min_runtime_mode(a: RuntimeMode, b: RuntimeMode) -> RuntimeMode {
    if a.rank() <= b.rank() { a } else { b }
}

fn min_interaction_mode(
    a: ProviderInteractionMode,
    b: ProviderInteractionMode,
) -> ProviderInteractionMode {
    if interaction_mode_rank(a) <= interaction_mode_rank(b) {
        a
    } else {
        b
    }
}

/// Intersects baseline approval, optional `x-legioncode-mode-limit` header, and optional managed policy cap.
///
/// Intersections are strictly monotonic narrowing: limits can only restrict and never widen.
pub fn intersect_mode_ceilings(
    approved: ModeCeiling,
    narrowed_to: Option<ModeCeiling>,
    managed_policy_max_runtime: Option<RuntimeMode>,
) -> ModeCeiling {
    let mut result = approved;
    if let Some(limit) = narrowed_to {
        result.runtime_mode = min_runtime_mode(result.runtime_mode, limit.runtime_mode);
        result.interaction_mode =
            min_interaction_mode(result.interaction_mode, limit.interaction_mode);
    }
    if let Some(policy_max) = managed_policy_max_runtime {
        result.runtime_mode = min_runtime_mode(result.runtime_mode, policy_max);
    }
    result
}

/// Returns `true` when `origin` is fenced to its own linked-thread slice (`!= FullAccess`).
pub fn link_origin_is_fenced(origin: &LinkOrigin) -> bool {
    origin.access != Some(McpClientAccess::FullAccess)
}

/// Enforces that a fenced MCP caller can only target threads matching its own `LinkOrigin.session_id`.
pub fn assert_same_link(
    caller_origin: Option<&LinkOrigin>,
    target_thread_link: Option<&LinkOrigin>,
) -> Result<(), McpAccessError> {
    let Some(caller) = caller_origin else {
        return Ok(());
    };
    if !link_origin_is_fenced(caller) {
        return Ok(());
    }
    match target_thread_link {
        Some(target) if target.session_id == caller.session_id => Ok(()),
        _ => Err(McpAccessError::OutsideLink {
            session_id: caller.session_id.clone(),
        }),
    }
}

/// Blocks host-only operations (projects, settings, scheduled tasks, peer forwarding) for fenced links.
pub fn assert_not_linked(
    caller_origin: Option<&LinkOrigin>,
    operation: &str,
) -> Result<(), McpAccessError> {
    let Some(caller) = caller_origin else {
        return Ok(());
    };
    if link_origin_is_fenced(caller) {
        return Err(McpAccessError::OperationForbiddenForLinkedClient {
            session_id: caller.session_id.clone(),
            operation: operation.to_owned(),
        });
    }
    Ok(())
}
