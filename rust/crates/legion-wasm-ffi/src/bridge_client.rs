//! Pure-Rust host-testable bridge for [`legion_client_core`] backing `WasmClientCore`.

use std::collections::BTreeMap;

use legion_client_core::backoff::{FailureReason, ReconnectBackoff, jittered_retry_delay_ms};
use legion_client_core::probe::SilentSocketStateMachine;
use legion_client_core::steady_shells::{
    EnvironmentShellStatus, STEADY_SHELL_RELEASE_WINDOW_MS, SteadyShellCoordinator,
    SteadyShellInput,
};
use legion_client_core::thread_families::{MountHandle, ThreadFamilyCache, ThreadFamilyRow};
use legion_contracts::ids::{EnvironmentId, ThreadId};

use crate::bridge_client_dto::{
    DisconnectDecisionResponse, ProbeStepRequest, ProbeStepResponse, SteadyShellEnqueuePayload,
    SteadyShellInputDto, SteadyShellStepResponse, TouchFamilyRequest,
};
use crate::bridge_client_family::touch_family_step;

/// Stateful host-testable client runtime core combining steady shells, thread families,
/// reconnect backoff, and silent-socket probing.
#[derive(Debug, Clone)]
pub struct ClientCoreBridge {
    last_now_ms: u64,
    steady: SteadyShellCoordinator<serde_json::Value>,
    steady_inputs: BTreeMap<EnvironmentId, SteadyShellInput<serde_json::Value>>,
    families: ThreadFamilyCache<ThreadFamilyRow<serde_json::Value>>,
    family_handles: BTreeMap<(EnvironmentId, ThreadId), Vec<MountHandle>>,
    backoff: ReconnectBackoff,
    probe: SilentSocketStateMachine,
}

impl ClientCoreBridge {
    /// Initializes a new [`ClientCoreBridge`] at explicit timestamp `now_ms`.
    pub fn new(now_ms: u64) -> Self {
        let mut steady = SteadyShellCoordinator::new(STEADY_SHELL_RELEASE_WINDOW_MS);
        steady.mount();
        Self {
            last_now_ms: now_ms,
            steady,
            steady_inputs: BTreeMap::new(),
            families: ThreadFamilyCache::default(),
            family_handles: BTreeMap::new(),
            backoff: ReconnectBackoff::new(),
            probe: SilentSocketStateMachine::new(None),
        }
    }

    /// Enqueues one or more environment shell updates and steps steady-shell release coordination.
    pub fn enqueue_steady_shell(&mut self, json: &str) -> Result<String, String> {
        let payload: SteadyShellEnqueuePayload = serde_json::from_str(json)
            .map_err(|err| format!("invalid steady shell JSON: {err}"))?;
        let (items, batch_now_ms) = match payload {
            SteadyShellEnqueuePayload::BatchList(items) => (items, None),
            SteadyShellEnqueuePayload::BatchObject { inputs, now_ms } => (inputs, now_ms),
            SteadyShellEnqueuePayload::Single(item) => {
                let item_now = item.now_ms;
                (vec![*item], item_now)
            }
        };
        if items.is_empty() {
            return Err("enqueue_steady_shell requires at least one input".to_owned());
        }

        let mut step_now_ms = batch_now_ms.unwrap_or(self.last_now_ms);
        for dto in items {
            if let Some(ts) = dto.now_ms {
                step_now_ms = step_now_ms.max(ts);
            }
            self.apply_steady_input(dto)?;
        }

        self.step_steady_at(step_now_ms)
    }

    /// Polls steady-shell release readiness at `now_ms`, releasing held rows once the 2s deadline
    /// expires.
    pub fn poll_steady_ready(&mut self, now_ms: u64) -> Result<String, String> {
        self.step_steady_at(now_ms)
    }

    /// Updates or queries the generation-keyed thread-family cache (`mount`, `touch`,
    /// `setGeneration`, `beginFetch`, `completeFetch`, `release`, `evict`, `merge`).
    pub fn touch_family(&mut self, json: &str) -> Result<String, String> {
        let req: TouchFamilyRequest = serde_json::from_str(json)
            .map_err(|err| format!("invalid touch_family JSON: {err}"))?;
        let now_ms = req.now_ms.unwrap_or(self.last_now_ms);
        self.last_now_ms = self.last_now_ms.max(now_ms);
        touch_family_step(&mut self.families, &mut self.family_handles, req, now_ms)
    }

    /// Records a transient disconnect at `now_ms` and returns the deterministic backoff decision.
    pub fn record_disconnect(&mut self, now_ms: u64) -> Result<String, String> {
        self.last_now_ms = self.last_now_ms.max(now_ms);
        let decision = self
            .backoff
            .record_failure(FailureReason::Transient, false, 0.5);
        let response = DisconnectDecisionResponse {
            now_ms,
            attempt: decision.attempt,
            delay_ms: decision.delay_ms,
            next_retry_at_ms: now_ms.saturating_add(decision.delay_ms),
            awaiting_approval: decision.awaiting_approval,
            first_loss: decision.first_loss,
            failure_count: self.backoff.failure_count(),
            backoff_failures: self.backoff.backoff_failures(),
        };
        serde_json::to_string(&response)
            .map_err(|err| format!("failed to serialize disconnect decision: {err}"))
    }

    /// Computes the deterministic midpoint-jittered reconnect delay in milliseconds for 0-based
    /// `attempt`.
    pub fn next_backoff_ms(&self, attempt: u32) -> u32 {
        let delay = jittered_retry_delay_ms(attempt, 0.5);
        u32::try_from(delay).unwrap_or(u32::MAX)
    }

    /// Advances the connected-lease [`SilentSocketStateMachine`] with one JSON step command.
    pub fn probe_step_json(&mut self, json: &str) -> Result<String, String> {
        let req: ProbeStepRequest =
            serde_json::from_str(json).map_err(|err| format!("invalid probe step JSON: {err}"))?;
        let actions = match req {
            ProbeStepRequest::Reset { route_kind } => {
                self.probe = SilentSocketStateMachine::new(route_kind);
                Vec::new()
            }
            ProbeStepRequest::Signal { signal, now_ms } => {
                let ts = self.advance_now(now_ms);
                self.probe.on_signal(signal, ts)
            }
            ProbeStepRequest::Tick {
                now_ms,
                current_frames,
            } => {
                let ts = self.advance_now(now_ms);
                self.probe.on_tick(ts, current_frames)
            }
            ProbeStepRequest::ConfigLoaded {
                loaded,
                current_frames,
                now_ms,
            } => {
                let ts = self.advance_now(now_ms);
                self.probe.on_config_loaded(loaded, current_frames, ts)
            }
            ProbeStepRequest::FramesObserved {
                current_frames,
                now_ms,
            } => {
                let ts = self.advance_now(now_ms);
                self.probe.on_frames_observed(current_frames, ts);
                Vec::new()
            }
            ProbeStepRequest::HostCheckResult {
                answered,
                current_frames,
                now_ms,
            } => {
                let ts = self.advance_now(now_ms);
                self.probe
                    .on_host_check_result(answered, current_frames, ts)
            }
            ProbeStepRequest::ProbeCompleted { succeeded } => {
                self.probe.on_probe_completed(succeeded)
            }
            ProbeStepRequest::Status => Vec::new(),
        };

        let response = ProbeStepResponse {
            actions,
            probing: self.probe.is_probing(),
            next_deadline_ms: self.probe.next_deadline_ms(),
        };
        serde_json::to_string(&response)
            .map_err(|err| format!("failed to serialize probe state: {err}"))
    }

    fn apply_steady_input(&mut self, dto: SteadyShellInputDto) -> Result<(), String> {
        let env_id = EnvironmentId::new(dto.environment_id).map_err(|err| err.to_string())?;
        if dto.remove {
            self.steady_inputs.remove(&env_id);
            return Ok(());
        }
        let status = dto.status.unwrap_or(if dto.snapshot.is_some() {
            EnvironmentShellStatus::Live
        } else {
            EnvironmentShellStatus::Empty
        });
        let input = SteadyShellInput::new(
            env_id.clone(),
            dto.snapshot,
            dto.server_sequence,
            status,
            dto.generation,
            dto.connecting.unwrap_or(true),
            dto.row_actions,
        );
        self.steady_inputs.insert(env_id, input);
        Ok(())
    }

    fn step_steady_at(&mut self, now_ms: u64) -> Result<String, String> {
        self.last_now_ms = self.last_now_ms.max(now_ms);
        let inputs: Vec<SteadyShellInput<serde_json::Value>> =
            self.steady_inputs.values().cloned().collect();
        let step = self.steady.step(&inputs, now_ms);
        let holding = self.steady.is_holding(&inputs);
        let response = SteadyShellStepResponse {
            now_ms,
            ready: !holding && step.release_at_ms.is_none(),
            changed: step.changed,
            holding,
            release_at_ms: step.release_at_ms,
            timer_token: step.timer_token,
            shells: &self.steady.state().shells,
        };
        serde_json::to_string(&response)
            .map_err(|err| format!("failed to serialize steady shell state: {err}"))
    }

    fn advance_now(&mut self, maybe_now: Option<u64>) -> u64 {
        let ts = maybe_now.unwrap_or(self.last_now_ms);
        self.last_now_ms = self.last_now_ms.max(ts);
        ts
    }
}
