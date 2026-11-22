//! Tests for reverse state transitions (`snooze`/`unsnooze`, `settle`/`unsettle`, `pin`/`unpin`, PR, `archive`/`unarchive`).

use legion_contracts::ids::{CommandId, ThreadId};
use legion_contracts::orchestration::{
    OrchestrationCommand, PatchValue, ThreadPullRequestLinkCommand, ThreadUnsettleCommand,
    UserActionReason,
};
use legion_contracts::pull_requests::ThreadPullRequestLinkSource;
use legion_orchestrator::sink::CommitCommandInput;
use legion_orchestrator::{DeciderError, OrchestratorError};

use super::helpers::{
    TestResult, create_thread_cmd, make_subagent_thread_event, new_test_orchestrator,
};

#[tokio::test]
async fn snooze_and_settle_support_reverse_states() -> TestResult {
    let orch = new_test_orchestrator()?;
    let t0 = "2026-11-20T00:00:00.000Z";
    let t1 = "2026-11-20T01:00:00.000Z";
    let tid = ThreadId::new("thread-rev-ss")?;

    orch.dispatch_command(
        &create_thread_cmd("c1", "thread-rev-ss", "proj-1", "Reverse")?,
        t0,
    )
    .await?;

    let past_snooze = OrchestrationCommand::ThreadSnooze {
        command_id: CommandId::new("c-snooze-bad")?,
        thread_id: tid.clone(),
        snoozed_until: "2026-11-19T23:59:00.000Z".to_owned(),
    };
    assert!(matches!(
        orch.dispatch_command(&past_snooze, t0).await,
        Err(OrchestratorError::Decider(
            DeciderError::InvalidSnoozeTimestamp { .. }
        ))
    ));

    orch.dispatch_command(
        &OrchestrationCommand::ThreadSnooze {
            command_id: CommandId::new("c-snooze")?,
            thread_id: tid.clone(),
            snoozed_until: "2026-11-21T00:00:00.000Z".to_owned(),
        },
        t0,
    )
    .await?;
    let unsnoozed = orch
        .dispatch_command(
            &OrchestrationCommand::ThreadUnsnooze {
                command_id: CommandId::new("c-unsnooze")?,
                thread_id: tid.clone(),
                reason: UserActionReason::User,
            },
            t1,
        )
        .await?;
    let th = unsnoozed.resulting_thread.ok_or("missing thread")?;
    assert!(th.snoozed_until.flatten().is_none());
    assert_eq!(th.last_snooze_wake_at.flatten().as_deref(), Some(t1));

    orch.dispatch_command(
        &OrchestrationCommand::ThreadSettle {
            command_id: CommandId::new("c-settle")?,
            thread_id: tid.clone(),
            expected_latest_run_id: None,
            settled_at: None,
        },
        t1,
    )
    .await?;
    let unsettled = orch
        .dispatch_command(
            &OrchestrationCommand::ThreadUnsettle(ThreadUnsettleCommand {
                command_id: CommandId::new("c-unsettle")?,
                thread_id: tid,
                reason: UserActionReason::User,
                restore_unsettled_at: PatchValue::default(),
            }),
            t1,
        )
        .await?;
    assert_eq!(
        unsettled
            .resulting_thread
            .as_ref()
            .and_then(|t| t.settled_override.as_deref()),
        Some("unsettled")
    );
    Ok(())
}

#[tokio::test]
async fn pin_and_pull_request_support_reverse_states() -> TestResult {
    let orch = new_test_orchestrator()?;
    let t1 = "2026-11-20T01:00:00.000Z";
    let tid = ThreadId::new("thread-rev-pp")?;

    orch.dispatch_command(
        &create_thread_cmd("c1", "thread-rev-pp", "proj-1", "Pin/PR")?,
        t1,
    )
    .await?;

    orch.dispatch_command(
        &OrchestrationCommand::ThreadPin {
            command_id: CommandId::new("c-pin")?,
            thread_id: tid.clone(),
            order_key: Some("k-1".to_owned()),
        },
        t1,
    )
    .await?;
    let unpinned = orch
        .dispatch_command(
            &OrchestrationCommand::ThreadUnpin {
                command_id: CommandId::new("c-unpin")?,
                thread_id: tid.clone(),
            },
            t1,
        )
        .await?;
    assert!(
        unpinned
            .resulting_thread
            .as_ref()
            .and_then(|t| t.pinned_at.as_ref())
            .and_then(|o| o.as_ref())
            .is_none()
    );

    orch.dispatch_command(
        &OrchestrationCommand::ThreadPullRequestLink(ThreadPullRequestLinkCommand {
            command_id: CommandId::new("c-pr-link")?,
            thread_id: tid.clone(),
            host: "github.com".to_owned(),
            repository: "acme/repo".to_owned(),
            number: 42,
            url: "https://github.com/acme/repo/pull/42".to_owned(),
            source: ThreadPullRequestLinkSource::Manual,
            repository_web_url: None,
        }),
        t1,
    )
    .await?;
    let unlinked = orch
        .dispatch_command(
            &OrchestrationCommand::ThreadPullRequestUnlink {
                command_id: CommandId::new("c-pr-unlink")?,
                thread_id: tid,
                host: "github.com".to_owned(),
                repository: "acme/repo".to_owned(),
                number: 42,
            },
            t1,
        )
        .await?;
    let pr_thread = unlinked.resulting_thread.ok_or("missing thread")?;
    assert_eq!(pr_thread.pull_requests.as_ref().map(Vec::len), Some(0));
    assert!(pr_thread.linked_pull_request.flatten().is_none());
    Ok(())
}

#[tokio::test]
async fn archive_guards_active_subagent_and_unarchive_reverses() -> TestResult {
    let orch = new_test_orchestrator()?;
    let t1 = "2026-11-20T01:00:00.000Z";
    let tid = ThreadId::new("thread-arch-parent")?;

    orch.dispatch_command(
        &create_thread_cmd("c-p", "thread-arch-parent", "proj-1", "Parent")?,
        t1,
    )
    .await?;
    let sub_ev = make_subagent_thread_event(
        "ev-sub-1",
        "thread-sub-1",
        "thread-arch-parent",
        "thread-arch-parent",
        "proj-1",
        t1,
    )?;
    orch.sink().commit_command(&CommitCommandInput {
        command_id: CommandId::new("c-sub-create")?,
        command_type: "thread.create".to_owned(),
        thread_id: ThreadId::new("thread-sub-1")?,
        actor_kind: "agent".to_owned(),
        causation_event_id: None,
        correlation_id: None,
        now_iso: t1.to_owned(),
        events: vec![sub_ev],
        outbox_effects: Vec::new(),
    })?;

    let blocked = orch
        .dispatch_command(
            &OrchestrationCommand::ThreadArchive {
                command_id: CommandId::new("c-arch-block")?,
                thread_id: tid.clone(),
            },
            t1,
        )
        .await;
    assert!(matches!(
        blocked,
        Err(OrchestratorError::Decider(
            DeciderError::ActiveWorkConflict { .. }
        ))
    ));

    orch.dispatch_command(
        &OrchestrationCommand::ThreadArchive {
            command_id: CommandId::new("c-arch-child")?,
            thread_id: ThreadId::new("thread-sub-1")?,
        },
        t1,
    )
    .await?;
    let archived = orch
        .dispatch_command(
            &OrchestrationCommand::ThreadArchive {
                command_id: CommandId::new("c-arch-parent")?,
                thread_id: tid.clone(),
            },
            t1,
        )
        .await?;
    assert!(
        archived
            .resulting_thread
            .and_then(|t| t.archived_at)
            .is_some()
    );

    let unarchived = orch
        .dispatch_command(
            &OrchestrationCommand::ThreadUnarchive {
                command_id: CommandId::new("c-unarch-parent")?,
                thread_id: tid,
            },
            t1,
        )
        .await?;
    assert!(
        unarchived
            .resulting_thread
            .and_then(|t| t.archived_at)
            .is_none()
    );
    Ok(())
}
