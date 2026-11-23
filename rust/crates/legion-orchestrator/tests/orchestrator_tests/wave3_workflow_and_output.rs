use legion_contracts::ids::{MessageId, ProjectId, ScheduledTaskId, ThreadId, TurnItemId};
use legion_contracts::orchestration::{
    CreationSource, OrchestrationActor, RunOutputSchema, RunStatus, TurnItem, TurnItemBase,
    TurnItemKind, TurnItemStatus,
};
use legion_contracts::pull_requests::{
    ThreadLinkedPullRequest, ThreadPullRequestKey, ThreadPullRequestLink,
    ThreadPullRequestLinkSource,
};
use legion_orchestrator::decider::output_ops::{
    pick_owed_output_run, pick_run_for_output_submission,
    pick_run_for_output_submission_with_context, resolve_until_output_across_turns,
};
use legion_orchestrator::projections::{
    WireShellOptions, list_wire_older_settled_threads, query_wire_shell_http_snapshot_with_options,
};
use legion_orchestrator::settlement_policy::{
    AutoSettleRetentionPlan, BranchPullRequestRef, BranchPullRequestState, DeferredPrLookupOutcome,
    PullRequestOwnershipIndex, TerminalSessionActivity, has_blocking_terminal_subprocess,
    plan_auto_settle_with_pr_lookup, resolve_deferred_pr_settlement,
};
use legion_orchestrator::sink::projection_queries::{
    query_provider_threads_for_thread, query_settled_todo_turn_items, query_unsettled_subagents,
};
use legion_orchestrator::workflow_lineage::{
    ProjectThreadListOptions, is_listed_thread_json, list_project_threads,
    strip_provenance_for_fork,
};
use legion_persistence::sqlite::SqliteStore;
use serde_json::json;

use super::helpers::TestResult;
use super::wave3_helpers::make_run;

#[test]
fn settlement_policy_stop_on_settle_pr_ownership_and_branch_deferral() -> TestResult {
    assert!(!has_blocking_terminal_subprocess(&[
        TerminalSessionActivity {
            status: "running".to_owned(),
            has_pid: true,
            stop_on_settle: true,
        }
    ]));
    let owned_link = ThreadPullRequestLink {
        host: "github.com".to_owned(),
        repository: "o/r".to_owned(),
        number: 55,
        url: "https://github.com/o/r/pull/55".to_owned(),
        repository_web_url: None,
        source: ThreadPullRequestLinkSource::Manual,
        linked_at: "2026-11-22T00:00:00.000Z".to_owned(),
        snapshot: None,
        stack: None,
        watch: None,
        ended_watch: None,
    };
    let idx = PullRequestOwnershipIndex::from_thread_links([std::slice::from_ref(&owned_link)]);
    let closed_pr = BranchPullRequestRef {
        key: ThreadPullRequestKey {
            host: "github.com".to_owned(),
            repository: "o/r".to_owned(),
            number: 55,
        },
        state: BranchPullRequestState::Merged,
    };
    assert!(idx.is_owned_by_another_thread(Some(&closed_pr)));
    assert!(idx.should_skip_branch_reference(
        None,
        Some(&ThreadLinkedPullRequest {
            project_id: ProjectId::new("p-1")?,
            repository: "o/r".to_owned(),
            number: 55,
            url: "https://github.com/o/r/pull/55".to_owned(),
        }),
    ));
    assert_eq!(
        plan_auto_settle_with_pr_lookup(true, true, Some("feat/x"), &[]),
        AutoSettleRetentionPlan::DeferForBranchLookup
    );
    assert!(matches!(
        resolve_deferred_pr_settlement(ThreadId::new("t-def")?, None),
        DeferredPrLookupOutcome::SettleNow { .. }
    ));
    Ok(())
}

fn insert_projection_thread(
    conn: &rusqlite::Connection,
    thread_id: &str,
    project_id: &str,
    title: &str,
    updated_at: &str,
    payload: &serde_json::Value,
) -> TestResult {
    conn.execute(
        "INSERT INTO orchestration_v2_projection_threads (
            thread_id, project_id, title, default_provider, runtime_mode,
            interaction_mode, created_at, updated_at, payload_json
         ) VALUES (?1, ?2, ?3, 'codex', 'full-access', 'default', ?4, ?4, ?5)",
        rusqlite::params![
            thread_id,
            project_id,
            title,
            updated_at,
            payload.to_string()
        ],
    )?;
    Ok(())
}

#[test]
fn v2_indexed_queries_and_workflow_thread_visibility_filter() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    insert_projection_thread(
        conn,
        "thr-main",
        "proj-1",
        "Main",
        "2026-11-22T10:00:00.000Z",
        &json!({
            "id": "thr-main",
            "projectId": "proj-1",
            "title": "Main",
            "updatedAt": "2026-11-22T10:00:00.000Z",
            "lineage": { "parentThreadId": null, "relationshipToParent": null, "rootThreadId": "thr-main" }
        }),
    )?;
    insert_projection_thread(
        conn,
        "thr-wf",
        "proj-1",
        "Step",
        "2026-11-22T10:01:00.000Z",
        &json!({
            "id": "thr-wf",
            "projectId": "proj-1",
            "title": "Step",
            "updatedAt": "2026-11-22T10:01:00.000Z",
            "settledOverride": "settled",
            "settledAt": "2026-11-22T09:00:00.000Z",
            "workflowRun": { "environmentId": "env-1", "runId": "wf-1", "workflow": "ci", "step": "test" },
            "lineage": { "parentThreadId": null, "relationshipToParent": null, "rootThreadId": "thr-wf", "workflowStep": true }
        }),
    )?;

    assert_eq!(query_unsettled_subagents(conn, "thr-main")?.len(), 0);
    assert_eq!(query_settled_todo_turn_items(conn, "thr-main")?.len(), 0);
    assert_eq!(
        query_provider_threads_for_thread(conn, "thr-main", true, None)?.len(),
        0
    );

    let listed_default = list_project_threads(
        conn,
        &ProjectThreadListOptions {
            project_id: Some("proj-1".to_owned()),
            include_subagents: false,
        },
    )?;
    assert_eq!(listed_default.len(), 1);
    assert_eq!(listed_default[0].id, "thr-main");

    let listed_with_wf = list_project_threads(
        conn,
        &ProjectThreadListOptions {
            project_id: Some("proj-1".to_owned()),
            include_subagents: true,
        },
    )?;
    assert_eq!(listed_with_wf.len(), 2);

    let snap = query_wire_shell_http_snapshot_with_options(
        conn,
        "env-1",
        WireShellOptions {
            omit_idle_subagents: false,
            omit_older_settled: false,
        },
    )?;
    assert_eq!(snap.threads.len(), 2);
    assert!(
        snap.threads
            .iter()
            .find(|t| t.id == "thr-wf")
            .and_then(|t| t.workflow_run.as_ref())
            .is_some()
    );
    assert_eq!(
        list_wire_older_settled_threads(conn, None, None, 10)?
            .threads
            .len(),
        0
    );

    let mut forked = json!({
        "id": "thr-wf",
        "delegatedFrom": { "threadId": "p" },
        "workflowRun": { "runId": "wf-1" },
        "lineage": { "workflowStep": true, "rootThreadId": "thr-wf" }
    });
    assert!(!is_listed_thread_json(&forked));
    strip_provenance_for_fork(&mut forked);
    assert!(is_listed_thread_json(&forked));
    Ok(())
}

#[test]
fn indexed_queries_filter_settled_todos_and_unsettled_subagents_on_populated_rows() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    conn.execute_batch(
        "INSERT INTO orchestration_v2_projection_runs (run_id, thread_id, ordinal, provider, status, requested_at, payload_json) VALUES
           ('r-done', 't-1', 1, 'codex', 'completed', '2026-11-22T00:00:00Z', '{}'),
           ('r-live', 't-1', 2, 'codex', 'running', '2026-11-22T00:01:00Z', '{}');
         INSERT INTO orchestration_v2_projection_turn_items (turn_item_id, thread_id, run_id, ordinal, type, status, updated_at, payload_json) VALUES
           ('ti-done', 't-1', 'r-done', 1, 'todo_list', 'completed', '2026-11-22T00:00:05Z', '{\"id\":\"ti-done\"}'),
           ('ti-live', 't-1', 'r-live', 2, 'todo_list', 'in_progress', '2026-11-22T00:01:05Z', '{\"id\":\"ti-live\"}');
         INSERT INTO orchestration_v2_projection_subagents (subagent_id, thread_id, run_id, parent_node_id, provider, origin, status, updated_at, payload_json, completion_delivery_state) VALUES
           ('s-settled', 't-1', 'r-done', 'n-0', 'codex', 'app_owned', 'completed', '2026-11-22T00:00:10Z', '{\"id\":\"s-settled\"}', 'delivered'),
           ('s-run-live', 't-1', 'r-live', 'n-0', 'codex', 'app_owned', 'completed', '2026-11-22T00:01:10Z', '{\"id\":\"s-run-live\"}', 'delivered');",
    )?;
    let todos = query_settled_todo_turn_items(conn, "t-1")?;
    assert_eq!(todos.len(), 1);
    assert_eq!(todos[0]["id"], "ti-done");
    let unsettled = query_unsettled_subagents(conn, "t-1")?;
    assert_eq!(unsettled.len(), 1);
    assert_eq!(unsettled[0]["id"], "s-run-live");
    Ok(())
}

#[test]
fn output_submission_newest_unanswered_and_scheduled_fresh_context_exemption() -> TestResult {
    let schema: RunOutputSchema = json!({ "type": "object" })
        .as_object()
        .cloned()
        .ok_or("expected JSON object schema")?;
    let mut older_asked = make_run("r-old", "thr-main", 1, RunStatus::Completed)?;
    older_asked.output_schema = Some(schema.clone());
    let mut newer_failed = make_run("r-new-fail", "thr-main", 2, RunStatus::Failed)?;
    newer_failed.output_schema = Some(schema);
    let wake_active = make_run("r-wake", "thr-main", 3, RunStatus::Running)?;
    assert!(pick_owed_output_run(&[older_asked.clone(), newer_failed.clone()], &[]).is_none());
    assert!(
        pick_run_for_output_submission(&[older_asked.clone(), newer_failed, wake_active.clone()])
            .is_err()
    );

    let mut r_sched = make_run("r-sched", "thr-main", 2, RunStatus::Completed)?;
    r_sched.fresh_context = Some(true);
    r_sched.user_message_id = MessageId::new("m-sched")?;
    let sched_turn_item = TurnItem {
        base: TurnItemBase {
            deferred_work: None,
            tool_non_execution_kind: None,
            tool_surface: None,
            tool_icon: None,
            tool_source: None,
            id: TurnItemId::new("ti-sched")?,
            thread_id: ThreadId::new("thr-main")?,
            run_id: Some(r_sched.id.clone()),
            node_id: None,
            provider_thread_id: None,
            provider_turn_id: None,
            native_item_ref: None,
            parent_item_id: None,
            ordinal: 1,
            status: TurnItemStatus::Completed,
            title: None,
            started_at: Some("2026-11-22T10:00:00.000Z".to_owned()),
            completed_at: None,
            updated_at: "2026-11-22T10:00:00.000Z".to_owned(),
        },
        kind: TurnItemKind::UserMessage {
            created_by: OrchestrationActor::System,
            creation_source: CreationSource::Server,
            message_id: MessageId::new("m-sched")?,
            scheduled_task_id: Some(ScheduledTaskId::new("st-1")?),
            sender_thread_id: None,
            linked_sender: None,
            idle_compaction: None,
            workflow_script: None,
            fresh_context: Some(true),
            input_intent: "schedule".to_owned(),
            text: "Run schedule".to_owned(),
            text_omitted: None,
            context: None,
            attachments: Vec::new(),
            model_switch: None,
        },
    };
    let runs_for_output = [older_asked, r_sched, wake_active];
    let picked = pick_run_for_output_submission_with_context(&runs_for_output, &[sched_turn_item])?;
    assert_eq!(picked.id.as_str(), "r-old");
    assert_eq!(
        resolve_until_output_across_turns(Some(json!({"ok":true})), None),
        Some(json!({"ok":true}))
    );
    Ok(())
}
