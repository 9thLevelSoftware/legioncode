//! Auth, project mutation, thread creation, and message-dispatch sub-reducers.

use serde_json::json;
use t3_contracts::ids::{CommandId, MessageId, ProjectId, ProviderInstanceId, ThreadId};
use t3_contracts::orchestration::{
    CreationSource, DispatchMode, MessageDispatchCommand, ModelSelection, OrchestrationActor,
    OrchestrationCommand, ProviderInteractionMode, ThreadCreateCommand,
};
use t3_contracts::policy::RuntimeMode;

use crate::actions::{ProjectInput, RightPaneTab};
use crate::effects::{WebHostEffect, WebUiError, encode_rpc_request_frame_parts};
use crate::state::WebAppState;
use crate::state_types::{ProjectSummary, ThreadSummary, ThreadTimeline, TimelineMessage};
use crate::update_palette::refresh_command_palette;

pub(crate) fn apply_pairing_success(
    state: &mut WebAppState,
    access_token: String,
    session_id: Option<String>,
    role: Option<String>,
    scopes: Vec<String>,
) -> Vec<WebHostEffect> {
    state.auth.bearer_token = Some(access_token.clone());
    state.auth.pairing_input.clear();
    state.auth.paired = true;
    state.auth.session_id = session_id;
    state.auth.role = role;
    state.auth.scopes = scopes;
    state.auth.last_error = None;
    vec![
        WebHostEffect::PersistSessionToken {
            token: access_token.clone(),
        },
        WebHostEffect::http_get("/api/projects", Some(access_token)),
        WebHostEffect::WsConnect {
            url: state.client.ws_url.clone(),
        },
    ]
}

pub(crate) fn apply_projects_snapshot(state: &mut WebAppState, projects: Vec<ProjectInput>) {
    state.projects = projects
        .into_iter()
        .map(|p| ProjectSummary {
            id: p.id,
            title: p.title,
            workspace_root: p.workspace_root,
            pinned_at: p.pinned_at,
            archived_at: p.archived_at,
            updated_at: p.updated_at,
        })
        .collect();
    if let Some(active) = state.active_project_id.as_deref()
        && !state.projects.iter().any(|p| p.id == active)
    {
        state.active_project_id = None;
    }
    if state.active_project_id.is_none() {
        state.active_project_id = state.projects.first().map(|p| p.id.clone());
    }
}

pub(crate) fn handle_submit_pairing(
    state: &mut WebAppState,
    client_label: Option<String>,
) -> Result<Vec<WebHostEffect>, WebUiError> {
    let token = state.auth.pairing_input.trim().to_owned();
    if token.is_empty() {
        state.auth.last_error = Some("missing pairing token".to_owned());
        return Ok(vec![WebHostEffect::FocusSelector {
            selector: "[data-testid=\"pairing-token-input\"]".to_owned(),
        }]);
    }
    let body = json!({
        "token": token,
        "clientLabel": client_label.unwrap_or_else(|| "t3-web-ui".to_owned()),
    });
    Ok(vec![WebHostEffect::http_post_json(
        "/api/auth/pair",
        &body,
        None,
    )?])
}

pub(crate) fn handle_create_project(
    state: &mut WebAppState,
    workspace_root: &str,
    title: Option<String>,
) -> Result<Vec<WebHostEffect>, WebUiError> {
    let ws = workspace_root.trim().to_owned();
    if ws.is_empty() {
        return Ok(vec![WebHostEffect::FocusSelector {
            selector: "[data-testid=\"project-workspace-input\"]".to_owned(),
        }]);
    }
    let mut payload = json!({
        "type": "project.create",
        "workspaceRoot": ws,
    });
    if let Some(t) = title.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty()) {
        payload["title"] = json!(t);
    }
    state.new_project_workspace_root.clear();
    state.new_project_title.clear();
    Ok(vec![WebHostEffect::http_post_json(
        "/api/projects/mutate",
        &payload,
        state.auth.bearer_token.clone(),
    )?])
}

pub(crate) fn handle_create_thread(
    state: &mut WebAppState,
    project_id: &str,
    title: &str,
    model: Option<String>,
) -> Result<Vec<WebHostEffect>, WebUiError> {
    let req_id = state.client.alloc_request_id();
    let tid_raw = format!("thread-{req_id}");
    let cmd_id = CommandId::new(format!("cmd-{req_id}"))
        .map_err(|e| WebUiError::InvalidId(e.to_string()))?;
    let tid = ThreadId::new(&tid_raw).map_err(|e| WebUiError::InvalidId(e.to_string()))?;
    let pid = ProjectId::new(project_id).map_err(|e| WebUiError::InvalidId(e.to_string()))?;
    let instance_id =
        ProviderInstanceId::new("codex").map_err(|e| WebUiError::InvalidId(e.to_string()))?;
    let model_name = model.unwrap_or_else(|| "gpt-5".to_owned());
    let cmd = OrchestrationCommand::ThreadCreate(ThreadCreateCommand {
        created_by: OrchestrationActor::User,
        creation_source: CreationSource::Web,
        command_id: cmd_id,
        thread_id: tid,
        project_id: pid,
        title: title.trim().to_owned(),
        model_selection: ModelSelection {
            instance_id,
            model: model_name.clone(),
            options: None,
        },
        runtime_mode: RuntimeMode::ApprovalRequired,
        interaction_mode: ProviderInteractionMode::Default,
        branch: None,
        worktree_path: None,
        link_origin: None,
        delegated_from: None,
        imported_native_thread: None,
        initial_message: None,
    });
    let payload = serde_json::to_value(&cmd)?;
    let parts =
        encode_rpc_request_frame_parts(req_id, "orchestration.dispatchCommand", &payload, None)?;

    state.threads.insert(
        tid_raw.clone(),
        ThreadSummary {
            id: tid_raw.clone(),
            project_id: project_id.to_owned(),
            title: title.trim().to_owned(),
            branch: None,
            model: Some(model_name),
            synchronized: false,
            last_sequence: 0,
        },
    );
    state.active_project_id = Some(project_id.to_owned());
    state.active_thread_id = Some(tid_raw);
    state.right_tab = RightPaneTab::Chat;
    refresh_command_palette(state);
    Ok(vec![WebHostEffect::WsSendFrameParts { parts }])
}

pub(crate) fn handle_send_message(
    state: &mut WebAppState,
) -> Result<Vec<WebHostEffect>, WebUiError> {
    let text = state.composer.draft.trim().to_owned();
    if text.is_empty() && state.composer.context_pills.is_empty() {
        return Ok(vec![WebHostEffect::FocusSelector {
            selector: "[data-testid=\"composer-input\"]".to_owned(),
        }]);
    }
    let Some(active_tid) = state.active_thread_id.clone() else {
        return Err(WebUiError::Validation(
            "select or create a thread before sending a message".to_owned(),
        ));
    };
    let req_id = state.client.alloc_request_id();
    let msg_id_raw = format!("msg-{req_id}");
    let cmd_id = CommandId::new(format!("cmd-{req_id}"))
        .map_err(|e| WebUiError::InvalidId(e.to_string()))?;
    let tid = ThreadId::new(&active_tid).map_err(|e| WebUiError::InvalidId(e.to_string()))?;
    let mid = MessageId::new(&msg_id_raw).map_err(|e| WebUiError::InvalidId(e.to_string()))?;
    let context_val = if state.composer.context_pills.is_empty() {
        None
    } else {
        Some(serde_json::to_value(&state.composer.context_pills)?)
    };
    let cmd = OrchestrationCommand::MessageDispatch(MessageDispatchCommand {
        notification: None,
        guest_message_id: None,
        created_by: OrchestrationActor::User,
        creation_source: CreationSource::Web,
        scheduled_task_id: None,
        sender_thread_id: None,
        linked_sender: None,
        idle_compaction: None,
        fresh_context: None,
        command_id: cmd_id,
        thread_id: tid,
        message_id: mid,
        text: text.clone(),
        context: context_val,
        attachments: Vec::new(),
        title_seed: None,
        model_selection: None,
        source_plan_ref: None,
        restart_continuation_of_run_id: None,
        usage_limit_continuation_of_run_id: None,
        manual_continuation_of_run_id: None,
        steer_follow_up_of_run_id: None,
        steer_follow_up_after_run_ordinal: None,
        usage_limit_recovery_request_id: None,
        delivery_intent: Some("auto".to_owned()),
        delegated_completion: None,
        dispatch_mode: DispatchMode::StartImmediately,
    });
    let payload = serde_json::to_value(&cmd)?;
    let parts =
        encode_rpc_request_frame_parts(req_id, "orchestration.dispatchCommand", &payload, None)?;

    let timeline = state
        .timelines
        .entry(active_tid.clone())
        .or_insert_with(|| ThreadTimeline {
            thread_id: active_tid,
            ..ThreadTimeline::default()
        });
    timeline.messages.push(TimelineMessage {
        id: msg_id_raw,
        role: "user".to_owned(),
        kind: "user_message".to_owned(),
        title: None,
        body: text,
        streaming: false,
        sequence: timeline.last_sequence,
        status: Some("pending".to_owned()),
    });
    state.composer.draft.clear();
    state.composer.context_pills.clear();

    Ok(vec![
        WebHostEffect::WsSendFrameParts { parts },
        WebHostEffect::FocusSelector {
            selector: "[data-testid=\"composer-input\"]".to_owned(),
        },
    ])
}
