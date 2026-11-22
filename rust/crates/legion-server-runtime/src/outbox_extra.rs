//! Steer, restart, detach, delegated-task stop, and provider-backed title handlers for `RuntimeOutboxExecutor`.

use std::path::PathBuf;
use std::time::Duration;

use legion_contracts::ids::{
    EnvironmentId, EventId, MessageId, ProviderDriverKind, ProviderSessionId, RunId, ThreadId,
};
use legion_contracts::orchestration::{
    DomainEvent, DomainEventPayload, ProviderSessionDetached, RunBackgroundWorkCancelled,
};
use legion_orchestrator::Orchestrator;
use legion_persistence::auth_records::list_peer_links;
use legion_providers::non_acp::grok_http::{GrokChatMessage, build_grok_chat_completions_request};
use legion_remote::live_network::live_peer_mcp::{LivePeerMcpOptions, LivePeerMcpSession};
use legion_remote::secrets::FileSecretStore;

use crate::error::ServerRuntimeError;
use crate::outbox_executor::ProviderDriverMode;
use crate::outbox_state::{LoadedThreadContext, derive_thread_title};
use crate::turn_events::commit_runtime_events;

pub(crate) fn wrap_system_event(
    thread_id: &ThreadId,
    run_id: Option<RunId>,
    now_iso: &str,
    payload: DomainEventPayload,
) -> Result<DomainEvent, ServerRuntimeError> {
    let id = EventId::new(format!("evt-{}", uuid::Uuid::new_v4()))
        .map_err(|e| ServerRuntimeError::Persistence(e.to_string()))?;
    Ok(DomainEvent {
        id,
        thread_id: thread_id.clone(),
        run_id,
        node_id: None,
        driver: None,
        provider_instance_id: None,
        raw_event_id: None,
        occurred_at: now_iso.to_owned(),
        payload,
    })
}

pub(crate) async fn handle_session_detach(
    orchestrator: &Orchestrator,
    non_acp_registry: &crate::non_acp_subprocess::NonAcpProcessRegistry,
    thread_id: &ThreadId,
    provider_session_id: &ProviderSessionId,
    detail: Option<&str>,
    now_iso: &str,
) -> Result<(), ServerRuntimeError> {
    let _ = non_acp_registry.detach_session(provider_session_id);
    let event = wrap_system_event(
        thread_id,
        None,
        now_iso,
        DomainEventPayload::ProviderSessionDetached(ProviderSessionDetached {
            provider_session_id: provider_session_id.clone(),
            detached_at: now_iso.to_owned(),
            reason: detail.map(ToOwned::to_owned),
        }),
    )?;
    commit_runtime_events(
        orchestrator.sink(),
        thread_id,
        "runtime.provider-session.detach",
        now_iso,
        vec![event],
        Vec::new(),
    )
}

pub(crate) async fn handle_delegated_tasks_stop(
    orchestrator: &Orchestrator,
    acp_registry: &crate::acp_registry::AcpProcessRegistry,
    non_acp_registry: &crate::non_acp_subprocess::NonAcpProcessRegistry,
    secrets_dir: Option<PathBuf>,
    thread_id: &ThreadId,
    reason: Option<&str>,
    now_iso: &str,
) -> Result<(), ServerRuntimeError> {
    let _ = acp_registry.interrupt_matching(thread_id, None, None, None)?;
    let _ = non_acp_registry.interrupt_matching(thread_id, None, None, None)?;

    if let Some(sec_dir) = secrets_dir {
        let peer_ids: Vec<String> = {
            let conn = orchestrator
                .sink()
                .db()
                .lock()
                .map_err(|_| ServerRuntimeError::Persistence("db mutex poisoned".to_owned()))?;
            list_peer_links(&conn)
                .unwrap_or_default()
                .into_iter()
                .map(|p| p.environment_id)
                .collect()
        };
        let secrets = FileSecretStore::new(sec_dir).map_err(|err| {
            ServerRuntimeError::Persistence(format!("secrets store init error: {err}"))
        })?;
        let probe_client = legion_remote::live_network::LivePeerClient::new()?;
        for env_id in peer_ids {
            let opts = LivePeerMcpOptions {
                client_name: "legion-server-runtime".to_owned(),
                client_version: crate::SERVER_RUNTIME_VERSION.to_owned(),
                mode_limit: None,
                sender_thread: EnvironmentId::new(&env_id)
                    .ok()
                    .map(|eid| (eid, thread_id.clone(), "legioncode".to_owned())),
                timeout: Duration::from_secs(5),
            };
            let creds = {
                let conn_guard =
                    orchestrator.sink().db().lock().map_err(|_| {
                        ServerRuntimeError::Persistence("db mutex poisoned".to_owned())
                    })?;
                legion_remote::peer::links::PeerLinksService::new(secrets.clone())
                    .resolve_credentials(&conn_guard, &env_id, now_iso)
                    .ok()
            };
            let Some(creds) = creds else {
                continue;
            };
            let mut chosen = None;
            for url in &creds.link.urls {
                if let Ok((origin, desc)) = probe_client.probe_environment(url).await
                    && desc.environment_id == creds.link.environment_id
                {
                    chosen = Some(origin);
                    break;
                }
            }
            let Some(target_url) = chosen.or_else(|| creds.link.urls.into_iter().next()) else {
                continue;
            };
            if let Ok(mut session) =
                LivePeerMcpSession::connect(&target_url, &creds.token, opts).await
            {
                let _ = session
                    .interrupt_remote_thread(&env_id, thread_id.as_str(), true, reason, None)
                    .await;
                let _ = session.close().await;
            }
        }
    }

    let ctx = {
        let conn = orchestrator
            .sink()
            .db()
            .lock()
            .map_err(|_| ServerRuntimeError::Persistence("db mutex poisoned".to_owned()))?;
        LoadedThreadContext::load(&conn, thread_id)?
    };
    if let Some(latest_run) = ctx.runs.last() {
        let evt = wrap_system_event(
            thread_id,
            Some(latest_run.id.clone()),
            now_iso,
            DomainEventPayload::RunBackgroundWorkCancelled(RunBackgroundWorkCancelled {
                run_id: latest_run.id.clone(),
                restart_cancelled_background_work: Vec::new(),
            }),
        )?;
        commit_runtime_events(
            orchestrator.sink(),
            thread_id,
            "runtime.delegated-tasks.stop",
            now_iso,
            vec![evt],
            Vec::new(),
        )?;
    }
    Ok(())
}

pub(crate) struct TitleGenerateRequest<'a> {
    pub orchestrator: &'a Orchestrator,
    pub ctx: LoadedThreadContext,
    pub title_provider: Option<ProviderDriverMode>,
    pub thread_id: &'a ThreadId,
    pub kind: &'a str,
    pub command_id: &'a str,
    pub message_id: Option<&'a MessageId>,
    pub now_iso: &'a str,
}

pub(crate) async fn handle_title_generate(
    req: TitleGenerateRequest<'_>,
) -> Result<(), ServerRuntimeError> {
    let TitleGenerateRequest {
        orchestrator,
        ctx,
        title_provider,
        thread_id,
        kind,
        command_id,
        message_id,
        now_iso,
    } = req;
    let request_id = ctx
        .thread
        .title_regeneration
        .as_ref()
        .and_then(|o| o.as_ref())
        .and_then(|v| v.get("requestId"))
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(command_id)
        .to_owned();
    let skip_initial = kind != "regenerate" && ctx.thread.title_renamed == Some(true);
    let source_text = message_id
        .map(|id| ctx.message_text(id))
        .filter(|s| !s.trim().is_empty())
        .or_else(|| ctx.messages_by_id.values().next().cloned())
        .unwrap_or_default();

    let new_title = if skip_initial || source_text.trim().is_empty() {
        None
    } else {
        let generated = match title_provider {
            Some(ProviderDriverMode::GrokHttp {
                endpoint_url,
                api_key,
                model,
            }) => {
                let eff_model = model.as_deref().unwrap_or("grok-3-mini");
                let base = (!endpoint_url.trim().is_empty()).then_some(endpoint_url.as_str());
                let spec = build_grok_chat_completions_request(
                    base,
                    &api_key,
                    eff_model,
                    &[GrokChatMessage {
                        role: "user".to_owned(),
                        content: format!(
                            "Summarize as a short 3-6 word thread title without quotes:\n{source_text}"
                        ),
                    }],
                    false,
                    Some(24),
                );
                fetch_short_grok_title(&spec).await.ok().flatten()
            }
            _ => None,
        };
        generated.or_else(|| derive_thread_title(&source_text))
    };

    let mut updated_thread = ctx.thread;
    let status = if let Some(title) = new_title {
        updated_thread.title = title;
        "renamed"
    } else {
        "unchanged"
    };
    now_iso.clone_into(&mut updated_thread.updated_at);
    updated_thread.title_regeneration = Some(None);
    updated_thread.last_title_regeneration = Some(Some(serde_json::json!({
        "status": status,
        "requestId": request_id,
        "completedAt": now_iso,
    })));
    let event = wrap_system_event(
        thread_id,
        None,
        now_iso,
        DomainEventPayload::ThreadState {
            event_type: "thread.metadata-updated".to_owned(),
            thread: Box::new(updated_thread),
        },
    )?;
    commit_runtime_events(
        orchestrator.sink(),
        thread_id,
        "runtime.thread.title-generate",
        now_iso,
        vec![event],
        Vec::new(),
    )
}

async fn fetch_short_grok_title(
    spec: &legion_providers::non_acp::NonAcpHttpRequestSpec,
) -> Result<Option<String>, ServerRuntimeError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| ServerRuntimeError::Upstream(e.to_string()))?;
    let mut req = client.post(&spec.url);
    for (k, v) in &spec.headers {
        req = req.header(k.as_str(), v.as_str());
    }
    if let Some(ref body) = spec.body {
        req = req.json(body);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| ServerRuntimeError::Upstream(e.to_string()))?;
    if !resp.status().is_success() {
        return Ok(None);
    }
    let json_val: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| ServerRuntimeError::Upstream(e.to_string()))?;
    let content = json_val
        .pointer("/choices/0/message/content")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.trim_matches('"').chars().take(60).collect::<String>());
    Ok(content)
}

pub(crate) fn _marker_unused_types() -> Option<(ProviderDriverKind, RunId)> {
    None
}
