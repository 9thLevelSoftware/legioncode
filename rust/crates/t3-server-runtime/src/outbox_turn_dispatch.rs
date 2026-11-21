//! Driver-specific `ProviderTurnStart` dispatch helper split from `outbox_executor`.

use std::path::PathBuf;

use t3_contracts::ids::{CheckpointScopeId, ProviderThreadId, RunId};
use t3_contracts::orchestration::{Run, RunAttempt};
use t3_contracts::policy::RuntimeMode;
use t3_orchestrator::Orchestrator;
use t3_providers::drivers::ProviderLaunchSpec;

use crate::acp_registry::{AcpDriverSpec, AcpProcessRegistry};
use crate::acp_subprocess::{AcpTurnInput, execute_acp_turn};
use crate::error::ServerRuntimeError;
use crate::non_acp_http::{NonAcpHttpDriver, NonAcpHttpTurnInput, execute_non_acp_http_turn};
use crate::non_acp_subprocess::{
    NonAcpProcessRegistry, NonAcpSubprocessInput, execute_non_acp_subprocess_turn,
};
use crate::non_acp_subprocess_proto::NonAcpSubprocessKind;
use crate::outbox_state::LoadedThreadContext;
use crate::replay_driver::{ReplayTurnInput, execute_dev_replay_turn};

/// Provider backend configured for a registered `provider_instance_id`.
#[derive(Debug, Clone)]
pub enum ProviderDriverMode {
    /// Deterministic local `Replay (dev)` driver.
    DevReplay,
    /// External ACP stdio subprocess CLI binary.
    AcpCli(AcpDriverSpec),
    /// `OpenAI` Codex App Server NDJSON-RPC subprocess.
    CodexAppServer(ProviderLaunchSpec),
    /// Anthropic Claude CLI `--output-format stream-json` subprocess.
    ClaudeStreamJson(ProviderLaunchSpec),
    /// Cursor agent sidecar JSON-lines subprocess.
    CursorSidecar(ProviderLaunchSpec),
    /// Pi coding agent CLI subprocess.
    PiCli(ProviderLaunchSpec),
    /// `OpenCode` HTTP session + SSE event stream.
    OpenCodeHttp {
        /// Base URL of the `OpenCode` HTTP server.
        base_url: String,
        /// Optional pre-encoded `Authorization` header.
        auth_header: Option<String>,
        /// Optional model override.
        model: Option<String>,
    },
    /// Grok / xAI OpenAI-compatible SSE streaming endpoint.
    GrokHttp {
        /// Optional endpoint override (`""` uses xAI default).
        endpoint_url: String,
        /// Bearer API key.
        api_key: String,
        /// Optional model override.
        model: Option<String>,
    },
}

pub(crate) struct TurnDispatchRequest<'a> {
    pub(crate) driver_mode: ProviderDriverMode,
    pub(crate) ctx: LoadedThreadContext,
    pub(crate) workspace_root: PathBuf,
    pub(crate) run_id: &'a RunId,
    pub(crate) now_iso: &'a str,
}

struct TurnSharedMeta {
    run: Run,
    attempt: Option<RunAttempt>,
    runtime_mode: RuntimeMode,
    workspace_root: PathBuf,
    prompt: String,
    provider_thread_id: Option<ProviderThreadId>,
    now_iso: String,
    scope_id: CheckpointScopeId,
    next_item_ordinal: u64,
    queue_title_generate: bool,
}

pub(crate) async fn dispatch_turn_start(
    orchestrator: &Orchestrator,
    acp_registry: &AcpProcessRegistry,
    non_acp_registry: &NonAcpProcessRegistry,
    req: TurnDispatchRequest<'_>,
) -> Result<(), ServerRuntimeError> {
    let run = req.ctx.find_run(req.run_id)?.clone();
    let attempt = req.ctx.find_attempt(&run);
    let prompt = req.ctx.message_text(&run.user_message_id);
    let scope_id = match req.ctx.scope_ids.first().cloned() {
        Some(existing) => existing,
        None => CheckpointScopeId::new("root")
            .map_err(|e| ServerRuntimeError::Persistence(e.to_string()))?,
    };
    let queue_title = req.ctx.thread.title_renamed != Some(true) && run.ordinal <= 1;
    let meta = TurnSharedMeta {
        run,
        attempt,
        runtime_mode: req.ctx.thread.runtime_mode,
        workspace_root: req.workspace_root,
        prompt,
        provider_thread_id: req.ctx.thread.active_provider_thread_id,
        now_iso: req.now_iso.to_owned(),
        scope_id,
        next_item_ordinal: req.ctx.next_item_ordinal,
        queue_title_generate: queue_title,
    };

    match req.driver_mode {
        ProviderDriverMode::DevReplay => {
            let input = ReplayTurnInput {
                run: meta.run,
                attempt: meta.attempt,
                workspace_root: meta.workspace_root.to_string_lossy().into_owned(),
                prompt: meta.prompt,
                provider_thread_id: meta.provider_thread_id,
                scenario: None,
                now_iso: meta.now_iso,
                scope_id: meta.scope_id,
                next_item_ordinal: meta.next_item_ordinal,
                queue_title_generate: meta.queue_title_generate,
            };
            execute_dev_replay_turn(orchestrator.sink(), &input)
        }
        ProviderDriverMode::AcpCli(spec) => {
            let input = AcpTurnInput {
                run: meta.run,
                attempt: meta.attempt,
                runtime_mode: meta.runtime_mode,
                interaction_mode: req.ctx.thread.interaction_mode,
                workspace_root: meta.workspace_root,
                prompt: meta.prompt,
                provider_thread_id: meta.provider_thread_id,
                now_iso: meta.now_iso,
                scope_id: meta.scope_id,
                next_item_ordinal: meta.next_item_ordinal,
                queue_title_generate: meta.queue_title_generate,
            };
            execute_acp_turn(orchestrator.sink(), acp_registry, &spec, &input).await
        }
        ProviderDriverMode::OpenCodeHttp {
            base_url,
            auth_header,
            model,
        } => {
            let drv = NonAcpHttpDriver::OpenCode {
                base_url,
                auth_header,
                model,
            };
            execute_http_mode(orchestrator, &drv, meta).await
        }
        ProviderDriverMode::GrokHttp {
            endpoint_url,
            api_key,
            model,
        } => {
            let drv = NonAcpHttpDriver::Grok {
                endpoint_url,
                api_key,
                model,
            };
            execute_http_mode(orchestrator, &drv, meta).await
        }
        other => execute_subprocess_mode(orchestrator, non_acp_registry, other, meta).await,
    }
}

async fn execute_http_mode(
    orchestrator: &Orchestrator,
    driver: &NonAcpHttpDriver,
    meta: TurnSharedMeta,
) -> Result<(), ServerRuntimeError> {
    let input = NonAcpHttpTurnInput {
        run: meta.run,
        attempt: meta.attempt,
        prompt: meta.prompt,
        provider_thread_id: meta.provider_thread_id,
        now_iso: meta.now_iso,
        scope_id: meta.scope_id,
        next_item_ordinal: meta.next_item_ordinal,
        queue_title_generate: meta.queue_title_generate,
    };
    execute_non_acp_http_turn(orchestrator.sink(), driver, &input).await
}

async fn execute_subprocess_mode(
    orchestrator: &Orchestrator,
    non_acp_registry: &NonAcpProcessRegistry,
    mode: ProviderDriverMode,
    meta: TurnSharedMeta,
) -> Result<(), ServerRuntimeError> {
    let (kind, launch) = match mode {
        ProviderDriverMode::CodexAppServer(launch) => {
            (NonAcpSubprocessKind::CodexAppServer, launch)
        }
        ProviderDriverMode::ClaudeStreamJson(launch) => {
            (NonAcpSubprocessKind::ClaudeStreamJson, launch)
        }
        ProviderDriverMode::CursorSidecar(launch) => (NonAcpSubprocessKind::CursorSidecar, launch),
        ProviderDriverMode::PiCli(launch) => (NonAcpSubprocessKind::PiCli, launch),
        _ => {
            return Err(ServerRuntimeError::Process(
                "unexpected driver mode in subprocess dispatch".to_owned(),
            ));
        }
    };
    let input = NonAcpSubprocessInput {
        kind,
        run: meta.run,
        attempt: meta.attempt,
        runtime_mode: meta.runtime_mode,
        workspace_root: meta.workspace_root,
        prompt: meta.prompt,
        provider_thread_id: meta.provider_thread_id,
        now_iso: meta.now_iso,
        scope_id: meta.scope_id,
        next_item_ordinal: meta.next_item_ordinal,
        queue_title_generate: meta.queue_title_generate,
    };
    execute_non_acp_subprocess_turn(orchestrator.sink(), non_acp_registry, &launch, &input).await
}
