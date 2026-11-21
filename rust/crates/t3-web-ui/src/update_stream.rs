//! WebSocket reconnect backoff, steady-shell (`BUG-21`), thread-family, and frame handlers.

use t3_client_core::backoff::FailureReason;
use t3_client_core::steady_shells::{EnvironmentShellStatus, SteadyShellInput};
use t3_client_core::thread_families::{ThreadFamilyRow, ThreadFamilySnapshot};
use t3_contracts::ids::{EnvironmentId, ThreadId};
use t3_contracts::orchestration::StreamItem;
use t3_shared::frame_parts::FrameOutput;

use crate::actions::{SteadyShellBatchItem, ThreadFamilyOp, ThreadSummaryInput};
use crate::effects::{WebHostEffect, WebUiError, encode_rpc_request_frame_parts};
use crate::state::{ConnectionStatus, WebAppState};
use crate::state_types::ThreadSummary;
use crate::update_projection::apply_stream_item;

pub(crate) fn handle_ws_connected(
    state: &mut WebAppState,
    now_ms: u64,
) -> Result<Vec<WebHostEffect>, WebUiError> {
    state.client.now_ms = state.client.now_ms.max(now_ms);
    state.client.connected_since_ms = Some(now_ms);
    state.client.next_retry_at_ms = None;
    state.client.last_backoff = None;
    state.client.reconnect_backoff.reset();
    state.client.connection_status = ConnectionStatus::Synchronizing;

    let req_id = state.client.alloc_request_id();
    let payload = serde_json::json!({
        "omitIdleSubagents": false,
        "omitOlderSettled": false,
        "includeArchived": false,
    });
    let parts =
        encode_rpc_request_frame_parts(req_id, "orchestration.getShellSnapshot", &payload, None)?;
    Ok(vec![WebHostEffect::WsSendFrameParts { parts }])
}

pub(crate) fn handle_ws_disconnected(
    state: &mut WebAppState,
    now_ms: u64,
    approval_pending: bool,
    random: f64,
) {
    state.client.now_ms = state.client.now_ms.max(now_ms);
    let established = state.client.connected_since_ms.is_some();
    if let Some(started) = state.client.connected_since_ms.take() {
        state
            .client
            .reconnect_backoff
            .on_session_ended(now_ms.saturating_sub(started));
    }
    let reason = if approval_pending {
        FailureReason::ApprovalPending
    } else {
        FailureReason::Transient
    };
    let decision = state
        .client
        .reconnect_backoff
        .record_failure(reason, established, random);
    state.client.last_backoff = Some(decision);
    state.client.next_retry_at_ms = Some(now_ms.saturating_add(decision.delay_ms));
    state.client.connection_status = if decision.awaiting_approval {
        ConnectionStatus::AwaitingApproval
    } else {
        ConnectionStatus::Disconnected
    };
}

pub(crate) fn handle_step_steady_shells(
    state: &mut WebAppState,
    now_ms: u64,
    items: Vec<SteadyShellBatchItem>,
) -> Result<(), WebUiError> {
    state.client.now_ms = state.client.now_ms.max(now_ms);
    for item in items {
        let env_id = EnvironmentId::new(&item.environment_id)
            .map_err(|e| WebUiError::InvalidId(e.to_string()))?;
        let snapshot = item.threads.map(|rows| {
            rows.into_iter()
                .map(|r| ThreadSummary {
                    id: r.id,
                    project_id: r.project_id,
                    title: r.title,
                    branch: r.branch,
                    model: r.model,
                    synchronized: item.status == EnvironmentShellStatus::Live,
                    last_sequence: item.server_sequence.unwrap_or(0),
                })
                .collect::<Vec<_>>()
        });
        let input = SteadyShellInput::new(
            env_id.clone(),
            snapshot,
            item.server_sequence,
            item.status,
            item.generation,
            item.connecting,
            item.row_actions,
        );
        state.client.steady_inputs.insert(env_id, input);
    }
    let batch: Vec<_> = state.client.steady_inputs.values().cloned().collect();
    let step = state.client.steady_shells.step(&batch, now_ms);
    state.client.last_steady_step = Some(step);
    sync_shown_threads_from_steady_shells(state);
    Ok(())
}

pub(crate) fn handle_poll_steady_shells(
    state: &mut WebAppState,
    now_ms: u64,
    timer_token: Option<u64>,
) {
    state.client.now_ms = state.client.now_ms.max(now_ms);
    let batch: Vec<_> = state.client.steady_inputs.values().cloned().collect();
    let step = match timer_token {
        Some(token) => state
            .client
            .steady_shells
            .on_release_timer(token, &batch, now_ms),
        None => Some(state.client.steady_shells.step(&batch, now_ms)),
    };
    if let Some(s) = step {
        state.client.last_steady_step = Some(s);
    }
    sync_shown_threads_from_steady_shells(state);
}

fn sync_shown_threads_from_steady_shells(state: &mut WebAppState) {
    let shown_rows: Vec<ThreadSummary> = state
        .client
        .steady_shells
        .state()
        .shells
        .values()
        .filter_map(|shell| shell.shown.as_ref())
        .flatten()
        .cloned()
        .collect();
    if shown_rows.is_empty() {
        return;
    }
    for row in shown_rows {
        let id = row.id.clone();
        state.threads.insert(id, row);
    }
    if state.active_thread_id.is_none() {
        state.active_thread_id = state.threads.keys().next().cloned();
    }
}

pub(crate) fn handle_touch_thread_family(
    state: &mut WebAppState,
    environment_id: &str,
    thread_id: &str,
    op: ThreadFamilyOp,
    now_ms: Option<u64>,
) -> Result<(), WebUiError> {
    let ts = now_ms.unwrap_or(state.client.now_ms);
    state.client.now_ms = state.client.now_ms.max(ts);
    let env_id =
        EnvironmentId::new(environment_id).map_err(|e| WebUiError::InvalidId(e.to_string()))?;
    let tid = ThreadId::new(thread_id).map_err(|e| WebUiError::InvalidId(e.to_string()))?;

    match op {
        ThreadFamilyOp::SetGeneration { generation } => {
            state
                .client
                .thread_families
                .set_shell_generation(&env_id, generation, ts);
        }
        ThreadFamilyOp::MountFamily => {
            let handle = state.client.thread_families.mount_family(&env_id, &tid, ts);
            state
                .client
                .family_handles
                .entry((env_id, tid))
                .or_default()
                .push(handle);
        }
        ThreadFamilyOp::BeginFetch => {
            let _ = state.client.thread_families.begin_fetch(&env_id, &tid, ts);
        }
        ThreadFamilyOp::CompleteFetch {
            generation,
            threads,
            snapshot_sequence,
            error,
        } => {
            let res = match error {
                Some(err) => Err(err),
                None => Ok(ThreadFamilySnapshot {
                    snapshot_sequence,
                    threads: build_family_rows(threads)?,
                }),
            };
            state
                .client
                .thread_families
                .complete_fetch(&env_id, &tid, generation, res, true, ts);
        }
        ThreadFamilyOp::Release => {
            if let Some(handles) = state.client.family_handles.get_mut(&(env_id, tid))
                && let Some(mut handle) = handles.pop()
            {
                let _ = state.client.thread_families.release(&mut handle, ts);
            }
        }
        ThreadFamilyOp::EvictExpired => {
            let _ = state.client.thread_families.evict_expired(ts);
        }
    }
    Ok(())
}

fn build_family_rows(
    threads: Vec<ThreadSummaryInput>,
) -> Result<Vec<ThreadFamilyRow<ThreadSummary>>, WebUiError> {
    let mut out = Vec::with_capacity(threads.len());
    for row in threads {
        let tid = ThreadId::new(&row.id).map_err(|e| WebUiError::InvalidId(e.to_string()))?;
        let summary = ThreadSummary {
            id: row.id,
            project_id: row.project_id,
            title: row.title,
            branch: row.branch,
            model: row.model,
            synchronized: false,
            last_sequence: 0,
        };
        out.push(ThreadFamilyRow::new(tid, summary));
    }
    Ok(out)
}

pub(crate) fn handle_receive_ws_frame(
    state: &mut WebAppState,
    raw_frame: &str,
    now_ms: Option<u64>,
) -> Result<Vec<WebHostEffect>, WebUiError> {
    if let Some(ts) = now_ms {
        state.client.now_ms = state.client.now_ms.max(ts);
    }
    let whole = match state.client.frame_assembler.push(raw_frame) {
        FrameOutput::IntermediateAck(ack) => {
            return Ok(vec![WebHostEffect::WsSendFrameParts {
                parts: vec![ack.to_owned()],
            }]);
        }
        FrameOutput::Complete(complete) => complete.into_owned(),
    };

    let val: serde_json::Value = serde_json::from_str(&whole)?;
    if val.get("kind").is_some() {
        let item: StreamItem = serde_json::from_value(val)
            .map_err(|err| WebUiError::InvalidStreamItem(err.to_string()))?;
        apply_stream_item(state, None, item);
        return Ok(Vec::new());
    }
    if let Some(items) = val.get("values").and_then(serde_json::Value::as_array) {
        for raw_item in items {
            if raw_item.get("kind").is_some() {
                let item: StreamItem = serde_json::from_value(raw_item.clone())
                    .map_err(|err| WebUiError::InvalidStreamItem(err.to_string()))?;
                apply_stream_item(state, None, item);
            }
        }
    }
    Ok(Vec::new())
}
