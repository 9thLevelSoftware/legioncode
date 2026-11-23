//! Wire shell-thread row assembly matching `ProjectionStore.ts` `shellFromState`.

use rusqlite::Connection;
use serde_json::{Value, json};

use crate::projections::ProjectionError;
use crate::projections::wire_codec::normalize_model_selection_wire;
use crate::projections::wire_shell_load::{
    ThreadShellContext, active_provider_goal, bool_at, compute_visible_counts,
    derive_blocked_on_thread_wait, empty_context, extract_pending_tasks, fork_source_thread_id,
    load_contexts_for_threads, load_raw_threads, opt_str_at, provider_instance_history, str_at,
    u64_at,
};
use crate::projections::wire_types::{WirePendingRuntimeRequestSummary, WireThreadShell};

pub(crate) fn load_wire_thread_shells(
    conn: &Connection,
    only_thread_id: Option<&str>,
    include_archived: bool,
) -> Result<Vec<WireThreadShell>, ProjectionError> {
    let mut filter_sql = String::from("WHERE deleted_at IS NULL");
    if !include_archived {
        filter_sql.push_str(" AND archived_at IS NULL");
    }
    let threads_map = load_raw_threads(conn, &filter_sql, only_thread_id)?;
    if threads_map.is_empty() {
        return Ok(Vec::new());
    }
    let mut all_threads = threads_map.clone();
    let mut pending_sources: Vec<String> = threads_map
        .values()
        .filter_map(fork_source_thread_id)
        .collect();
    while let Some(source_id) = pending_sources.pop() {
        if all_threads.contains_key(&source_id) {
            continue;
        }
        let loaded = load_raw_threads(conn, "WHERE 1=1", Some(&source_id))?;
        if let Some(src_val) = loaded.into_values().next() {
            if let Some(next_src) = fork_source_thread_id(&src_val) {
                pending_sources.push(next_src);
            }
            all_threads.insert(source_id, src_val);
        }
    }

    let ids: Vec<String> = all_threads.keys().cloned().collect();
    let mut contexts = load_contexts_for_threads(conn, &ids)?;
    let visible_counts = compute_visible_counts(&all_threads, &contexts);

    let mut out = Vec::with_capacity(threads_map.len());
    for (tid, thread_val) in threads_map {
        let ctx = contexts.remove(&tid).unwrap_or_else(empty_context);
        let visible_item_count = visible_counts
            .get(&tid)
            .copied()
            .unwrap_or(ctx.visible_local_count);
        out.push(build_wire_thread_shell(
            &thread_val,
            &ctx,
            visible_item_count,
        ));
    }
    out.sort_by(|a, b| {
        b.pinned_at
            .is_some()
            .cmp(&a.pinned_at.is_some())
            .then_with(|| b.updated_at.cmp(&a.updated_at))
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(out)
}

type ErrorSummaryTuple = (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

struct ShellRunContext<'a> {
    provider_instance_id: String,
    latest_run: Option<&'a Value>,
    active_run: Option<&'a Value>,
    activity_run: Option<&'a Value>,
    pending_runtime_request: Option<WirePendingRuntimeRequestSummary>,
    latest_user_message_at: Option<String>,
    latest_user_authored_message_at: Option<String>,
    errors: ErrorSummaryTuple,
    settled_override: Option<String>,
    pending_guest_message_count: u64,
    tags: Option<Vec<String>>,
}

fn derive_shell_run_context<'a>(t: &Value, ctx: &'a ThreadShellContext) -> ShellRunContext<'a> {
    let provider_instance_id = str_at(t, "providerInstanceId")
        .unwrap_or("codex")
        .to_owned();
    let provider_session = ctx
        .provider_sessions
        .iter()
        .filter(|s| str_at(s, "providerInstanceId") == Some(provider_instance_id.as_str()))
        .max_by(|a, b| {
            str_at(a, "updatedAt")
                .unwrap_or("")
                .cmp(str_at(b, "updatedAt").unwrap_or(""))
        });
    let visible_runs: Vec<&Value> = ctx.runs.iter().filter(|r| !is_silent_run(r, ctx)).collect();
    let latest_run = visible_runs
        .iter()
        .copied()
        .rev()
        .find(|r| str_at(r, "status") != Some("queued") || bool_at(r, "queueHeld") != Some(true))
        .or_else(|| visible_runs.last().copied());
    let active_run = visible_runs
        .iter()
        .copied()
        .filter(|r| {
            matches!(
                str_at(r, "status"),
                Some("preparing" | "starting" | "running")
            )
        })
        .max_by_key(|r| u64_at(r, "ordinal"));
    let activity_run = visible_runs
        .iter()
        .copied()
        .filter(|r| {
            matches!(
                str_at(r, "status"),
                Some("preparing" | "starting" | "running" | "waiting")
            )
        })
        .max_by_key(|r| u64_at(r, "ordinal"));
    let pending_runtime_request = pick_pending_request(ctx, activity_run.is_some());
    let mut user_msgs: Vec<&Value> = ctx
        .messages
        .iter()
        .filter(|m| {
            str_at(m, "role") == Some("user") && m.get("idleCompaction").is_none_or(Value::is_null)
        })
        .collect();
    user_msgs.sort_by(|a, b| {
        str_at(b, "updatedAt")
            .unwrap_or("")
            .cmp(str_at(a, "updatedAt").unwrap_or(""))
    });
    let errors = extract_error_summary(latest_run, ctx, provider_session);
    let settled_override = match str_at(t, "settledOverride") {
        Some("settled") => Some("settled".to_owned()),
        Some("active" | "unsettled") => Some("active".to_owned()),
        _ => None,
    };
    let pending_guest_message_count = t
        .get("guestShare")
        .and_then(|g| g.get("pendingMessages"))
        .and_then(Value::as_array)
        .map_or(0, |arr| arr.len() as u64);
    let tags = t
        .get("tags")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .filter(|v| !v.is_empty());
    ShellRunContext {
        provider_instance_id,
        latest_run,
        active_run,
        activity_run,
        pending_runtime_request,
        latest_user_message_at: user_msgs.first().and_then(|m| opt_str_at(m, "updatedAt")),
        latest_user_authored_message_at: user_msgs
            .iter()
            .find(|m| str_at(m, "createdBy") == Some("user"))
            .and_then(|m| opt_str_at(m, "updatedAt")),
        errors,
        settled_override,
        pending_guest_message_count,
        tags,
    }
}

fn build_wire_thread_shell(
    t: &Value,
    ctx: &ThreadShellContext,
    visible_item_count: u64,
) -> WireThreadShell {
    let rc = derive_shell_run_context(t, ctx);
    let provider_instance_id = rc.provider_instance_id.clone();

    WireThreadShell {
        created_by: str_at(t, "createdBy").unwrap_or("user").to_owned(),
        creation_source: str_at(t, "creationSource").unwrap_or("web").to_owned(),
        id: str_at(t, "id").unwrap_or("").to_owned(),
        project_id: str_at(t, "projectId").unwrap_or("").to_owned(),
        title: str_at(t, "title").unwrap_or("").to_owned(),
        provider_instance_id: rc.provider_instance_id,
        model_selection: t.get("modelSelection").cloned().map_or_else(
            || json!({ "instanceId": provider_instance_id, "model": "unknown" }),
            normalize_model_selection_wire,
        ),
        runtime_mode: str_at(t, "runtimeMode").unwrap_or("full-access").to_owned(),
        interaction_mode: str_at(t, "interactionMode").unwrap_or("default").to_owned(),
        branch: opt_str_at(t, "branch"),
        worktree_path: opt_str_at(t, "worktreePath"),
        pull_requests: t
            .get("pullRequests")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        linked_pull_request: t.get("linkedPullRequest").cloned(),
        branch_pull_request: t.get("branchPullRequest").cloned(),
        issues: t.get("issues").and_then(Value::as_array).cloned(),
        active_order_key: t.get("activeOrderKey").cloned().filter(|v| !v.is_null()),
        lineage: t.get("lineage").cloned().unwrap_or_else(|| json!({})),
        forked_from: t.get("forkedFrom").cloned(),
        active_provider_thread_id: opt_str_at(t, "activeProviderThreadId"),
        history_origin: opt_str_at(t, "historyOrigin"),
        link_origin: t.get("linkOrigin").cloned().filter(|v| !v.is_null()),
        delegated_from: t.get("delegatedFrom").cloned().filter(|v| !v.is_null()),
        latest_run_id: rc.latest_run.and_then(|r| opt_str_at(r, "id")),
        latest_run_requested_at: rc.latest_run.and_then(|r| opt_str_at(r, "requestedAt")),
        latest_run_started_at: rc.latest_run.and_then(|r| opt_str_at(r, "startedAt")),
        latest_run_completed_at: rc.latest_run.and_then(|r| opt_str_at(r, "completedAt")),
        latest_run_stop_requested: rc.latest_run.is_some_and(|r| {
            let rid = str_at(r, "id");
            ctx.turn_items.iter().any(|item| {
                str_at(item, "runId") == rid
                    && str_at(item, "type") == Some("run_interrupt_request")
            })
        }),
        latest_run_background_wake: rc
            .latest_run
            .is_some_and(|r| is_background_wake_run(r, ctx)),
        active_run_id: rc.active_run.and_then(|r| opt_str_at(r, "id")),
        blocked_on_thread_wait: derive_blocked_on_thread_wait(rc.active_run, &ctx.turn_items),
        activity_run_status: rc.activity_run.and_then(|r| opt_str_at(r, "status")),
        activity_run_started_at: rc.activity_run.and_then(|r| {
            opt_str_at(r, "workStartedAt")
                .or_else(|| opt_str_at(r, "startedAt"))
                .or_else(|| opt_str_at(r, "requestedAt"))
        }),
        status: rc
            .latest_run
            .and_then(|r| str_at(r, "status"))
            .unwrap_or("idle")
            .to_owned(),
        last_error: rc.errors.0,
        last_error_class: rc.errors.1,
        last_error_code: rc.errors.2,
        usage_limit_reset_at: rc.errors.3,
        pending_runtime_request: rc.pending_runtime_request,
        pending_guest_message_count: rc.pending_guest_message_count,
        latest_visible_message: None,
        latest_user_message_at: rc.latest_user_message_at,
        latest_user_authored_message_at: rc.latest_user_authored_message_at,
        has_actionable_proposed_plan: ctx.has_active_proposed_plan,
        pending_background_tasks: extract_pending_tasks(ctx),
        provider_instance_history: provider_instance_history(
            str_at(t, "id").unwrap_or(""),
            &ctx.provider_threads,
        ),
        goal: active_provider_goal(t, &ctx.provider_threads),
        item_count: ctx.visible_local_count,
        visible_item_count,
        created_at: str_at(t, "createdAt").unwrap_or("").to_owned(),
        updated_at: str_at(t, "updatedAt").unwrap_or("").to_owned(),
        archived_at: opt_str_at(t, "archivedAt"),
        settled_override: rc.settled_override,
        settled_at: opt_str_at(t, "settledAt"),
        unsettled_at: opt_str_at(t, "unsettledAt"),
        snoozed_until: opt_str_at(t, "snoozedUntil"),
        snoozed_at: opt_str_at(t, "snoozedAt"),
        pinned_at: opt_str_at(t, "pinnedAt"),
        auto_settle_disabled_at: opt_str_at(t, "autoSettleDisabledAt"),
        pin_order_key: opt_str_at(t, "pinOrderKey"),
        tags: rc.tags,
        last_visited_at: opt_str_at(t, "lastVisitedAt"),
        opens: t.get("opens").cloned().filter(|v| !v.is_null()),
        title_regeneration: t
            .get("titleRegeneration")
            .and_then(crate::projections::wire_codec::sanitize_title_regeneration_value),
        last_title_regeneration: t
            .get("lastTitleRegeneration")
            .and_then(crate::projections::wire_codec::sanitize_last_title_regeneration_value),
        limit_recovery: t.get("limitRecovery").cloned().filter(|v| !v.is_null()),
        deleted_at: opt_str_at(t, "deletedAt"),
    }
}

fn is_silent_run(run: &Value, ctx: &ThreadShellContext) -> bool {
    if str_at(run, "status") == Some("failed") {
        return false;
    }
    let msg_id = str_at(run, "userMessageId");
    let run_id = str_at(run, "id");
    ctx.messages
        .iter()
        .any(|m| str_at(m, "id") == msg_id && m.get("idleCompaction").is_some_and(|v| !v.is_null()))
        && !ctx.turn_items.iter().any(|i| {
            str_at(i, "runId") == run_id
                && str_at(i, "type") == Some("compaction")
                && str_at(i, "status") == Some("failed")
        })
}

fn is_background_wake_run(run: &Value, ctx: &ThreadShellContext) -> bool {
    let msg_id = str_at(run, "userMessageId");
    let run_id = str_at(run, "id");
    let Some(msg) = ctx.messages.iter().find(|m| str_at(m, "id") == msg_id) else {
        return false;
    };
    let is_wake = msg.get("delegatedCompletion").is_none_or(Value::is_null)
        && msg.get("notification").and_then(|n| str_at(n, "outcome")) == Some("updated")
        && matches!(
            msg.get("notification")
                .and_then(|n| n.get("source"))
                .and_then(|s| str_at(s, "kind")),
            Some("background_task" | "command" | "monitor")
        );
    is_wake
        && !ctx
            .messages
            .iter()
            .any(|m| str_at(m, "runId") == run_id && str_at(m, "createdBy") == Some("user"))
}

fn pick_pending_request(
    ctx: &ThreadShellContext,
    has_activity_run: bool,
) -> Option<WirePendingRuntimeRequestSummary> {
    let mut best = ctx
        .runtime_requests
        .first()
        .map(|r| WirePendingRuntimeRequestSummary {
            id: str_at(r, "id").unwrap_or("").to_owned(),
            kind: str_at(r, "kind").unwrap_or("approval").to_owned(),
            created_at: str_at(r, "createdAt").unwrap_or("").to_owned(),
            blocking: Some(
                r.get("responseCapability").and_then(|c| str_at(c, "type")) != Some("message"),
            ),
            user_only: None,
        });
    if has_activity_run
        && let Some(item) = ctx.turn_items.iter().rev().find(|i| {
            str_at(i, "status") == Some("waiting")
                && matches!(str_at(i, "type"), Some("secret_request" | "link_request"))
                && i.get("nodeId").is_some_and(|v| !v.is_null())
        })
    {
        let card = WirePendingRuntimeRequestSummary {
            id: str_at(item, "id").unwrap_or("").to_owned(),
            kind: str_at(item, "type").unwrap_or("secret_request").to_owned(),
            created_at: str_at(item, "updatedAt").unwrap_or("").to_owned(),
            blocking: Some(str_at(item, "type") == Some("secret_request")),
            user_only: Some(true),
        };
        if best.is_none()
            || best
                .as_ref()
                .is_some_and(|b| card.created_at >= b.created_at)
        {
            best = Some(card);
        }
    }
    best
}

fn extract_error_summary(
    latest_run: Option<&Value>,
    ctx: &ThreadShellContext,
    provider_session: Option<&Value>,
) -> ErrorSummaryTuple {
    if let Some(run) = latest_run
        && str_at(run, "status") == Some("failed")
    {
        let rid = str_at(run, "id");
        let root_node = str_at(run, "rootNodeId");
        if let Some(err_item) = ctx.turn_items.iter().rev().find(|i| {
            str_at(i, "runId") == rid
                && str_at(i, "type") == Some("error")
                && str_at(i, "status") == Some("failed")
                && (root_node.is_none() || str_at(i, "nodeId") == root_node)
        }) {
            let failure = err_item.get("failure");
            return (
                opt_str_at(err_item, "message"),
                failure.and_then(|f| opt_str_at(f, "class")),
                failure.and_then(|f| opt_str_at(f, "code")),
                failure.and_then(|f| opt_str_at(f, "resetAt")),
            );
        }
    }
    let session_err = provider_session.and_then(|s| opt_str_at(s, "lastError"));
    (session_err, None, None, None)
}
