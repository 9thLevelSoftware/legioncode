//! Tests for `ShellSnapshot` (`shellThreadFamilies` + `shellOlderSettled` 25-thread cut).

use std::collections::HashSet;

use legion_contracts::ids::{CommandId, ProjectId, ThreadId};
use legion_contracts::orchestration::{DispatchMode, OrchestrationCommand};
use legion_orchestrator::Orchestrator;
use legion_orchestrator::projections::{
    SHELL_NEWEST_SETTLED_KEPT, ShellSnapshotOptions, list_older_settled_threads,
};
use legion_orchestrator::sink::CommitCommandInput;
use legion_orchestrator::time::format_millis_iso;

use super::helpers::{
    TestResult, create_thread_cmd, make_subagent_thread_event, new_test_orchestrator,
    send_message_cmd,
};

async fn create_settled_thread(
    orch: &Orchestrator,
    cmd_prefix: &str,
    tid: &str,
    project: &str,
    ts: &str,
) -> TestResult {
    orch.dispatch_command(
        &create_thread_cmd(&format!("{cmd_prefix}-c"), tid, project, tid)?,
        ts,
    )
    .await?;
    orch.dispatch_command(
        &OrchestrationCommand::ThreadSettle {
            command_id: CommandId::new(format!("{cmd_prefix}-s"))?,
            thread_id: ThreadId::new(tid)?,
            expected_latest_run_id: None,
            settled_at: Some(ts.to_owned()),
        },
        ts,
    )
    .await?;
    Ok(())
}

async fn seed_shell_fixture(orch: &Orchestrator, base_ms: i64, now_ts: &str) -> TestResult {
    for idx in 1..=28_i64 {
        let rank_ts = if idx == 4 {
            base_ms + 5 * 1_000
        } else {
            base_ms + idx * 1_000
        };
        let ts = format_millis_iso(rank_ts);
        let tid = format!("t-settled-{idx:02}");
        create_settled_thread(orch, &format!("c-{idx:02}"), &tid, "proj-1", &ts).await?;
    }

    for (idx, tid, prefix) in [(1_i64, "t-old-p2-a", "cp2a"), (2_i64, "t-old-p2-b", "cp2b")] {
        let ts = format_millis_iso(base_ms - idx * 60_000);
        create_settled_thread(orch, prefix, tid, "proj-2", &ts).await?;
    }

    let old_ts = format_millis_iso(base_ms - 120_000);
    let future_snooze = format_millis_iso(base_ms + 3_600_000);
    create_settled_thread(orch, "cpin", "t-old-pinned", "proj-1", &old_ts).await?;
    orch.dispatch_command(
        &OrchestrationCommand::ThreadPin {
            command_id: CommandId::new("cpin-p")?,
            thread_id: ThreadId::new("t-old-pinned")?,
            order_key: None,
        },
        &old_ts,
    )
    .await?;

    create_settled_thread(orch, "csnz", "t-old-snoozed", "proj-1", &old_ts).await?;
    orch.dispatch_command(
        &OrchestrationCommand::ThreadSnooze {
            command_id: CommandId::new("csnz-z")?,
            thread_id: ThreadId::new("t-old-snoozed")?,
            snoozed_until: future_snooze,
        },
        now_ts,
    )
    .await?;

    orch.dispatch_command(
        &create_thread_cmd("clive-root", "t-live-root", "proj-1", "Live Root")?,
        now_ts,
    )
    .await?;
    for (sub_id, live) in [("sub-live", true), ("sub-idle", false)] {
        let sub_ev = make_subagent_thread_event(
            &format!("ev-{sub_id}"),
            sub_id,
            "t-live-root",
            "t-live-root",
            "proj-1",
            now_ts,
        )?;
        orch.sink().commit_command(&CommitCommandInput {
            command_id: CommandId::new(format!("c-{sub_id}"))?,
            command_type: "thread.create".to_owned(),
            thread_id: ThreadId::new(sub_id)?,
            actor_kind: "agent".to_owned(),
            causation_event_id: None,
            correlation_id: None,
            now_iso: now_ts.to_owned(),
            events: vec![sub_ev],
            outbox_effects: Vec::new(),
        })?;
        if live {
            orch.dispatch_command(
                &send_message_cmd(
                    &format!("m-{sub_id}"),
                    sub_id,
                    &format!("msg-{sub_id}"),
                    "Active child work",
                    DispatchMode::StartImmediately,
                )?,
                now_ts,
            )
            .await?;
        }
    }
    Ok(())
}

#[tokio::test]
async fn shell_snapshot_enforces_families_and_25_older_settled_cut() -> TestResult {
    let orch = new_test_orchestrator()?;
    let base_ms: i64 = 1_795_132_800_000;
    let now_ts = format_millis_iso(base_ms + 40_000);
    seed_shell_fixture(&orch, base_ms, &now_ts).await?;

    let snap = orch.shell_snapshot(&now_ts, ShellSnapshotOptions::default())?;
    assert_eq!(
        snap.shell_older_settled.newest_settled_kept,
        SHELL_NEWEST_SETTLED_KEPT
    );
    let ids: HashSet<String> = snap
        .threads
        .iter()
        .map(|t| t.thread.id.as_str().to_owned())
        .collect();

    assert!(ids.contains("t-settled-28"));
    assert!(ids.contains("t-settled-04"));
    assert!(!ids.contains("t-settled-03"));
    assert!(!ids.contains("t-old-p2-a"));
    assert!(ids.contains("t-old-pinned"));
    assert!(ids.contains("t-old-snoozed"));
    assert!(ids.contains("t-live-root"));
    assert!(ids.contains("sub-live"));
    assert!(!ids.contains("sub-idle"));

    let live_family = snap
        .shell_thread_families
        .iter()
        .find(|f| f.root.thread.id.as_str() == "t-live-root")
        .ok_or("missing t-live-root family")?;
    assert_eq!(live_family.children.len(), 1);
    assert_eq!(live_family.children[0].thread.id.as_str(), "sub-live");

    assert_eq!(snap.shell_older_settled.total_omitted_count, 5);
    assert_eq!(snap.shell_older_settled.total_older_settled_count, 7);

    let conn = orch.sink().db().lock().map_err(|e| e.to_string())?;
    let p2_older = list_older_settled_threads(&conn, Some(&ProjectId::new("proj-2")?), &now_ts)?;
    assert_eq!(p2_older.len(), 2);
    Ok(())
}
