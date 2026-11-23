//! ACP `session/request_permission` types and policy selection.

use legion_contracts::orchestration::{ProviderApprovalDecision, ProviderInteractionMode};
use legion_contracts::policy::RuntimeMode;
use serde::{Deserialize, Serialize};

use super::presentation::{ToolCallKind, ToolCallUpdatePayload};

/// Permission choice semantics offered by an ACP agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionOptionKind {
    /// Approve this single invocation.
    AllowOnce,
    /// Approve and remember for the session/policy.
    AllowAlways,
    /// Deny this single invocation.
    RejectOnce,
    /// Deny and remember rejection.
    RejectAlways,
    /// Unrecognized option kind.
    #[serde(untagged)]
    Unknown(String),
}

/// Selectable permission option in `session/request_permission`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionOption {
    /// Agent option ID echoed back in [`RequestPermissionOutcome::Selected`].
    pub option_id: String,
    /// Human-readable label.
    pub name: String,
    /// Semantic decision kind.
    pub kind: PermissionOptionKind,
}

/// Agent-to-client `session/request_permission` parameters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestPermissionRequest {
    /// Active ACP session ID.
    pub session_id: String,
    /// Tool call requesting authorization.
    pub tool_call: ToolCallUpdatePayload,
    /// Choices offered by the agent.
    pub options: Vec<PermissionOption>,
}

/// Outcome returned in [`RequestPermissionResponse`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum RequestPermissionOutcome {
    /// Permission request was cancelled.
    Cancelled,
    /// User or policy selected a concrete option ID.
    Selected {
        /// Selected [`PermissionOption::option_id`].
        #[serde(rename = "optionId")]
        option_id: String,
    },
}

/// Response payload for ACP `session/request_permission`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestPermissionResponse {
    /// Resolution outcome for the permission prompt.
    pub outcome: RequestPermissionOutcome,
}

/// Evaluates an ACP `session/request_permission` request against explicit user
/// decisions or runtime/interaction mode policy.
///
/// In `Plan` interaction mode, mutating tool kinds (`Edit`, `Delete`, `Move`,
/// `Execute`) are always denied even if `RuntimeMode::FullAccess` is active.
///
/// # Examples
///
/// ```
/// use legion_contracts::orchestration::{ProviderApprovalDecision, ProviderInteractionMode};
/// use legion_contracts::policy::RuntimeMode;
/// use legion_providers::acp::{
///     PermissionOption, PermissionOptionKind, RequestPermissionOutcome,
///     RequestPermissionRequest, ToolCallKind, ToolCallUpdatePayload,
///     select_permission_option,
/// };
///
/// let req = RequestPermissionRequest {
///     session_id: "sess-1".to_owned(),
///     tool_call: ToolCallUpdatePayload {
///         tool_call_id: "tc-1".to_owned(),
///         title: Some("Write file".to_owned()),
///         kind: Some(ToolCallKind::Edit),
///         status: None,
///         locations: None,
///         content: None,
///         raw_input: None,
///         raw_output: None,
///     },
///     options: vec![
///         PermissionOption {
///             option_id: "allow".to_owned(),
///             name: "Allow once".to_owned(),
///             kind: PermissionOptionKind::AllowOnce,
///         },
///         PermissionOption {
///             option_id: "reject".to_owned(),
///             name: "Reject".to_owned(),
///             kind: PermissionOptionKind::RejectOnce,
///         },
///     ],
/// };
/// let resp = select_permission_option(
///     &req,
///     None,
///     Some(RuntimeMode::FullAccess),
///     Some(ProviderInteractionMode::Plan),
/// );
/// assert_eq!(
///     resp.outcome,
///     RequestPermissionOutcome::Selected { option_id: "reject".to_owned() }
/// );
/// ```
#[must_use]
pub fn select_permission_option(
    request: &RequestPermissionRequest,
    decision: Option<ProviderApprovalDecision>,
    runtime_mode: Option<RuntimeMode>,
    interaction_mode: Option<ProviderInteractionMode>,
) -> RequestPermissionResponse {
    let pick = |kinds: &[PermissionOptionKind]| {
        kinds
            .iter()
            .find_map(|target| {
                request
                    .options
                    .iter()
                    .find(|opt| &opt.kind == target)
                    .map(|opt| opt.option_id.clone())
            })
            .map_or(RequestPermissionOutcome::Cancelled, |option_id| {
                RequestPermissionOutcome::Selected { option_id }
            })
    };

    let allow_once_first = [
        PermissionOptionKind::AllowOnce,
        PermissionOptionKind::AllowAlways,
    ];
    let allow_always_first = [
        PermissionOptionKind::AllowAlways,
        PermissionOptionKind::AllowOnce,
    ];
    let reject_once_first = [
        PermissionOptionKind::RejectOnce,
        PermissionOptionKind::RejectAlways,
    ];

    if interaction_mode == Some(ProviderInteractionMode::Plan)
        && is_mutating_tool(&request.tool_call)
    {
        return RequestPermissionResponse {
            outcome: pick(&reject_once_first),
        };
    }

    let outcome = match decision {
        Some(ProviderApprovalDecision::Accept) => pick(&allow_once_first),
        Some(
            ProviderApprovalDecision::AcceptAlways | ProviderApprovalDecision::AcceptForSession,
        ) => pick(&allow_always_first),
        Some(ProviderApprovalDecision::Decline) => pick(&reject_once_first),
        Some(ProviderApprovalDecision::Cancel) => RequestPermissionOutcome::Cancelled,
        None => match runtime_mode {
            Some(RuntimeMode::FullAccess | RuntimeMode::Auto) => pick(&allow_once_first),
            Some(RuntimeMode::AutoAcceptEdits)
                if !matches!(request.tool_call.kind, Some(ToolCallKind::Execute)) =>
            {
                pick(&allow_once_first)
            }
            _ => pick(&reject_once_first),
        },
    };
    RequestPermissionResponse { outcome }
}

const fn is_mutating_tool(tool_call: &ToolCallUpdatePayload) -> bool {
    matches!(
        tool_call.kind,
        Some(
            ToolCallKind::Edit | ToolCallKind::Delete | ToolCallKind::Move | ToolCallKind::Execute
        )
    )
}
