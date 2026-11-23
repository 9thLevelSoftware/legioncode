//! Helper mapping functions shared by [`super::orchestration`].

use legion_contracts::orchestration::{AppThread, RunStatus};
use legion_contracts::pull_requests::ThreadPullRequestKey;
use serde_json::Value;

use super::types::{
    MilestoneActor, MilestoneEnvelope, MilestoneGuestResolution, MilestoneId, MilestoneInputKind,
    MilestoneInputResolution, MilestonePrOutcome, MilestoneRunOutcome,
};

pub(super) fn envelope(
    id: MilestoneId,
    thread: &AppThread,
    occurred_at: &str,
    actor: MilestoneActor,
    workflow_run_id: Option<&str>,
) -> MilestoneEnvelope {
    MilestoneEnvelope {
        id,
        occurred_at: occurred_at.to_owned(),
        project_id: Some(thread.project_id.clone()),
        thread_id: Some(thread.id.clone()),
        parent_thread_id: thread.lineage.parent_thread_id.clone(),
        actor,
        is_subagent_child: thread.lineage.relationship_to_parent.as_deref() == Some("subagent"),
        workflow_run_id: workflow_run_id.map(str::to_owned),
    }
}

pub(super) fn input_kind(kind: &str) -> Option<MilestoneInputKind> {
    match kind {
        "user_input" | "mcp-elicitation" => Some(MilestoneInputKind::UserInput),
        "command" | "file-read" | "file-change" | "permission" => {
            Some(MilestoneInputKind::Approval)
        }
        _ => None,
    }
}

pub(super) const fn map_run_outcome(status: RunStatus) -> Option<MilestoneRunOutcome> {
    match status {
        RunStatus::Completed => Some(MilestoneRunOutcome::Completed),
        RunStatus::Failed => Some(MilestoneRunOutcome::Failed),
        RunStatus::Interrupted => Some(MilestoneRunOutcome::Interrupted),
        RunStatus::Cancelled => Some(MilestoneRunOutcome::Cancelled),
        RunStatus::Preparing
        | RunStatus::Queued
        | RunStatus::Starting
        | RunStatus::Running
        | RunStatus::Waiting
        | RunStatus::RolledBack => None,
    }
}

pub(super) fn map_input_resolution(status: &str) -> Option<MilestoneInputResolution> {
    match status {
        "resolved" => Some(MilestoneInputResolution::Resolved),
        "expired" => Some(MilestoneInputResolution::Expired),
        "cancelled" => Some(MilestoneInputResolution::Cancelled),
        _ => None,
    }
}

pub(super) fn map_guest_resolution(status: &str) -> Option<MilestoneGuestResolution> {
    match status {
        "sent" => Some(MilestoneGuestResolution::Sent),
        "dismissed" => Some(MilestoneGuestResolution::Dismissed),
        _ => None,
    }
}

pub(super) fn parse_pr_outcome(raw: &str) -> MilestonePrOutcome {
    match raw {
        "completed" => MilestonePrOutcome::Completed,
        "failed" => MilestonePrOutcome::Failed,
        "cancelled" => MilestonePrOutcome::Cancelled,
        "updated" => MilestonePrOutcome::Updated,
        _ => MilestonePrOutcome::Unknown,
    }
}

pub(super) fn parse_pr_key(value: &Value) -> Option<ThreadPullRequestKey> {
    let host = value.get("host")?.as_str()?.to_owned();
    let repository = value.get("repository")?.as_str()?.to_owned();
    let number = u32::try_from(value.get("number")?.as_u64()?).ok()?;
    Some(ThreadPullRequestKey {
        host,
        repository,
        number,
    })
}
