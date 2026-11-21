//! `StreamItem` projection and `DomainEvent` timeline handlers (`BUG-18`).

use t3_contracts::orchestration::{
    DomainEvent, DomainEventPayload, StreamItem, ThreadProjection, TurnItem,
};

use crate::state::{ConnectionStatus, WebAppState};
use crate::state_types::{ThreadSummary, ThreadTimeline, TimelineMessage};
use crate::update_timeline::{
    refresh_activity_summary, turn_item_to_timeline, upsert_timeline_message,
};

pub(crate) fn apply_stream_item(
    state: &mut WebAppState,
    thread_id_hint: Option<&str>,
    item: StreamItem,
) {
    match item {
        StreamItem::Synchronized => {
            state.client.connection_status = ConnectionStatus::Live;
            let target_id = thread_id_hint
                .map(str::to_owned)
                .or_else(|| state.active_thread_id.clone());
            if let Some(tid) = target_id {
                if let Some(thread) = state.threads.get_mut(&tid) {
                    thread.synchronized = true;
                }
                state
                    .timelines
                    .entry(tid.clone())
                    .or_insert_with(|| ThreadTimeline {
                        thread_id: tid,
                        ..ThreadTimeline::default()
                    })
                    .synchronized = true;
            }
        }
        StreamItem::Snapshot {
            snapshot_sequence,
            projection,
            ..
        } => {
            apply_thread_projection(state, snapshot_sequence, *projection);
        }
        StreamItem::Event { sequence, event } => {
            apply_domain_event(state, sequence, *event);
        }
        StreamItem::UnknownEvent { sequence, .. } => {
            if let Some(tid) = thread_id_hint
                .map(str::to_owned)
                .or_else(|| state.active_thread_id.clone())
            {
                let timeline =
                    state
                        .timelines
                        .entry(tid.clone())
                        .or_insert_with(|| ThreadTimeline {
                            thread_id: tid,
                            ..ThreadTimeline::default()
                        });
                timeline.last_sequence = timeline.last_sequence.max(sequence);
                timeline.unknown_event_count = timeline.unknown_event_count.saturating_add(1);
            }
        }
    }
    refresh_activity_summary(state);
}

fn apply_thread_projection(
    state: &mut WebAppState,
    snapshot_sequence: u64,
    projection: ThreadProjection,
) {
    let tid = projection.thread.id.as_str().to_owned();
    let summary = ThreadSummary {
        id: tid.clone(),
        project_id: projection.thread.project_id.as_str().to_owned(),
        title: projection.thread.title.clone(),
        branch: projection.thread.branch.clone(),
        model: Some(projection.thread.model_selection.model.clone()),
        synchronized: true,
        last_sequence: snapshot_sequence,
    };
    state.threads.insert(tid.clone(), summary);
    if state.active_thread_id.is_none() {
        state.active_thread_id = Some(tid.clone());
    }
    if state.active_project_id.is_none() {
        state.active_project_id = Some(projection.thread.project_id.as_str().to_owned());
    }

    let mut messages = Vec::new();
    for msg in projection.messages {
        messages.push(TimelineMessage {
            id: msg.id.as_str().to_owned(),
            role: msg.role,
            kind: "conversation_message".to_owned(),
            title: None,
            body: msg.text,
            streaming: msg.streaming,
            sequence: snapshot_sequence,
            status: None,
        });
    }
    let items: Vec<TurnItem> = if projection.visible_turn_items.is_empty() {
        projection.turn_items
    } else {
        projection
            .visible_turn_items
            .into_iter()
            .map(|p| p.item)
            .collect()
    };
    for item in items {
        upsert_timeline_message(
            &mut messages,
            turn_item_to_timeline(item, snapshot_sequence),
        );
    }

    let active_run_status = projection.runs.last().map(|r| format!("{:?}", r.status));
    state.timelines.insert(
        tid.clone(),
        ThreadTimeline {
            thread_id: tid,
            last_sequence: snapshot_sequence,
            synchronized: true,
            unknown_event_count: 0,
            active_run_status,
            messages,
        },
    );
    if let Some(env_id) = state.client.environment_id.as_ref() {
        let next_gen = state.client.thread_families.shell_generation(env_id).max(1);
        state
            .client
            .thread_families
            .set_shell_generation(env_id, next_gen, state.client.now_ms);
    }
}

fn apply_domain_event(state: &mut WebAppState, sequence: u64, event: DomainEvent) {
    let tid = event.thread_id.as_str().to_owned();
    let timeline = state
        .timelines
        .entry(tid.clone())
        .or_insert_with(|| ThreadTimeline {
            thread_id: tid.clone(),
            ..ThreadTimeline::default()
        });
    timeline.last_sequence = timeline.last_sequence.max(sequence);

    match event.payload {
        DomainEventPayload::ThreadState { thread, .. } => {
            let sync = state.threads.get(&tid).is_some_and(|t| t.synchronized);
            state.threads.insert(
                tid.clone(),
                ThreadSummary {
                    id: tid.clone(),
                    project_id: thread.project_id.as_str().to_owned(),
                    title: thread.title,
                    branch: thread.branch,
                    model: Some(thread.model_selection.model),
                    synchronized: sync,
                    last_sequence: sequence,
                },
            );
            if state.active_thread_id.is_none() {
                state.active_thread_id = Some(tid);
            }
        }
        DomainEventPayload::MessageUpdated(msg) => {
            let entry = TimelineMessage {
                id: msg.id.as_str().to_owned(),
                role: msg.role,
                kind: "conversation_message".to_owned(),
                title: None,
                body: msg.text,
                streaming: msg.streaming,
                sequence,
                status: None,
            };
            upsert_timeline_message(&mut timeline.messages, entry);
        }
        DomainEventPayload::TurnItemUpdated(item) => {
            sync_turn_item_features(state, &tid, &item, sequence);
            let tl = state.timelines.entry(tid).or_default();
            upsert_timeline_message(&mut tl.messages, turn_item_to_timeline(*item, sequence));
        }
        DomainEventPayload::RunUpdated { run, .. } => {
            timeline.active_run_status = Some(format!("{:?}", run.status).to_ascii_lowercase());
        }
        DomainEventPayload::PlanUpdated(plan) => {
            let entry = TimelineMessage {
                id: plan.id.as_str().to_owned(),
                role: "assistant".to_owned(),
                kind: "plan".to_owned(),
                title: Some("Plan".to_owned()),
                body: plan.markdown.or(plan.explanation).unwrap_or_default(),
                streaming: false,
                sequence,
                status: Some(plan.status),
            };
            upsert_timeline_message(&mut timeline.messages, entry);
        }
        DomainEventPayload::CheckpointCaptured(cp) => {
            let entry = TimelineMessage {
                id: cp.id.as_str().to_owned(),
                role: "system".to_owned(),
                kind: "checkpoint".to_owned(),
                title: Some(format!("Checkpoint #{}", cp.ordinal_within_scope)),
                body: cp.r#ref.as_str().to_owned(),
                streaming: false,
                sequence,
                status: Some(cp.status),
            };
            upsert_timeline_message(&mut timeline.messages, entry);
        }
        _ => {}
    }
}

fn sync_turn_item_features(
    state: &mut WebAppState,
    thread_id: &str,
    item: &t3_contracts::orchestration::TurnItem,
    sequence: u64,
) {
    use t3_contracts::orchestration::TurnItemKind;
    let item_id = item.base.id.as_str();
    match &item.kind {
        TurnItemKind::Error { failure, .. } => {
            let key = format!("{thread_id}:{item_id}:{sequence}");
            crate::update_features::record_thread_error(
                state,
                thread_id,
                &failure.message,
                crate::state_features::ThreadErrorSource::Runtime,
                Some(&key),
                false,
            );
        }
        TurnItemKind::SecretRequest {
            label,
            reason,
            url,
            placeholder,
            delivery,
            fields,
            ..
        } => {
            let payload = crate::actions_features::UpsertSecretCardPayload {
                thread_id: thread_id.to_owned(),
                turn_item_id: item_id.to_owned(),
                label: label.clone(),
                placeholder: placeholder.clone(),
                reason: Some(reason.clone()),
                url: url.clone(),
                delivery_file: delivery.as_deref() == Some("file"),
                fields: fields.clone(),
            };
            crate::update_features::upsert_secret_card(state, &payload);
        }
        TurnItemKind::SystemNotice {
            mcp_sign_in: Some(notice),
            ..
        } => {
            crate::update_features::upsert_mcp_sign_in(state, thread_id, item_id, notice);
        }
        _ => {}
    }
}
