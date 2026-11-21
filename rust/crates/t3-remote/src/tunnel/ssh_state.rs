//! SSH local port-forward tunnel lifecycle state machine (`packages/ssh/src/tunnel.ts`).

use super::types::{
    RemoteServerKind, SshTunnelState, TUNNEL_STDERR_TAIL_CHARS, TunnelAction, TunnelEndpoints,
    TunnelEvent, TunnelTransport,
};
use super::{
    BatchMode, SshError, build_control_master_cancel_args, build_control_master_tunnel_args,
    build_direct_tunnel_args, is_ssh_agent_signing_failure, is_ssh_auth_failure,
    target_connection_key,
};

/// Deterministic state machine for SSH local port-forward tunnel management.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshTunnelStateMachine {
    state: SshTunnelState,
    batch_mode: BatchMode,
}

impl Default for SshTunnelStateMachine {
    fn default() -> Self {
        Self::new(BatchMode::No)
    }
}

impl SshTunnelStateMachine {
    /// Creates a new tunnel state machine in [`SshTunnelState::Idle`].
    #[must_use]
    pub fn new(batch_mode: BatchMode) -> Self {
        Self {
            state: SshTunnelState::Idle,
            batch_mode,
        }
    }

    /// Returns the current [`SshTunnelState`].
    #[must_use]
    pub fn state(&self) -> &SshTunnelState {
        &self.state
    }

    /// Processes `event` and returns the resulting [`TunnelAction`]s.
    ///
    /// # Errors
    ///
    /// Returns [`SshError::InvalidStateTransition`] or [`SshError::InvalidTarget`] when `event` is invalid.
    pub fn handle(&mut self, event: TunnelEvent) -> Result<Vec<TunnelAction>, SshError> {
        match event {
            TunnelEvent::BeginResolve(target) => {
                if !matches!(self.state, SshTunnelState::Idle) {
                    return Err(invalid_transition(&self.state, "BeginResolve"));
                }
                self.state = SshTunnelState::Resolving {
                    target: target.clone(),
                };
                Ok(vec![TunnelAction::ResolveTarget(target)])
            }
            TunnelEvent::ResolveSucceeded(resolved) => {
                if !matches!(self.state, SshTunnelState::Resolving { .. }) {
                    return Err(invalid_transition(&self.state, "ResolveSucceeded"));
                }
                let key = target_connection_key(&resolved);
                self.state = SshTunnelState::LaunchingRemote {
                    key,
                    target: resolved.clone(),
                };
                Ok(vec![TunnelAction::LaunchRemoteServer { target: resolved }])
            }
            TunnelEvent::RemoteLaunched {
                remote_port,
                local_port,
                remote_server_kind,
            } => self.on_remote_launched(remote_port, local_port, remote_server_kind),
            TunnelEvent::StderrChunk(chunk) => {
                if let SshTunnelState::StartingForward { stderr_tail, .. } = &mut self.state {
                    append_stderr_tail(stderr_tail, &chunk);
                }
                Ok(Vec::new())
            }
            TunnelEvent::AuthFailed {
                error_message,
                prompt_available,
                live_control_path,
            } => self.on_auth_failed(&error_message, prompt_available, live_control_path),
            TunnelEvent::ForwardListening => self.on_forward_listening(),
            TunnelEvent::HttpReady => self.on_http_ready(),
            TunnelEvent::StaleDetected => self.begin_close(false),
            TunnelEvent::Disconnect => self.begin_close(true),
            TunnelEvent::ProcessExited { exit_code } => self.on_process_exited(exit_code),
            TunnelEvent::RemoteShuttingDown { pid } => self.on_remote_shutting_down(pid),
            TunnelEvent::ShutdownCompleted => {
                if !matches!(self.state, SshTunnelState::Closing { .. }) {
                    return Err(invalid_transition(&self.state, "ShutdownCompleted"));
                }
                self.state = SshTunnelState::Closed;
                Ok(Vec::new())
            }
        }
    }

    fn on_remote_launched(
        &mut self,
        remote_port: u16,
        local_port: u16,
        remote_server_kind: Option<RemoteServerKind>,
    ) -> Result<Vec<TunnelAction>, SshError> {
        let SshTunnelState::LaunchingRemote { key, target } = &self.state else {
            return Err(invalid_transition(&self.state, "RemoteLaunched"));
        };
        let args = build_direct_tunnel_args(target, local_port, remote_port, self.batch_mode)?;
        self.state = SshTunnelState::StartingForward {
            key: key.clone(),
            target: target.clone(),
            local_port,
            remote_port,
            remote_server_kind,
            transport: TunnelTransport::Direct,
            auth_prompts: 0,
            control_fallback_used: false,
            stderr_tail: String::new(),
        };
        Ok(vec![
            TunnelAction::SpawnForward {
                args,
                transport: TunnelTransport::Direct,
            },
            TunnelAction::WaitForForward { local_port },
        ])
    }

    fn on_auth_failed(
        &mut self,
        error_message: &str,
        prompt_available: bool,
        live_control_path: Option<String>,
    ) -> Result<Vec<TunnelAction>, SshError> {
        let SshTunnelState::StartingForward {
            target,
            local_port,
            remote_port,
            transport,
            auth_prompts,
            control_fallback_used,
            stderr_tail,
            ..
        } = &mut self.state
        else {
            return Err(invalid_transition(&self.state, "AuthFailed"));
        };
        if !is_ssh_auth_failure(error_message) {
            let tail = stderr_tail.clone();
            self.state = SshTunnelState::Failed {
                message: error_message.to_owned(),
                stderr_tail: tail,
            };
            return Ok(Vec::new());
        }
        if matches!(transport, TunnelTransport::Direct)
            && !*control_fallback_used
            && let Some(control_path) = live_control_path
        {
            *control_fallback_used = true;
            let cm_transport = TunnelTransport::ControlMaster {
                control_path: control_path.clone(),
            };
            let args =
                build_control_master_tunnel_args(target, &control_path, *local_port, *remote_port)?;
            *transport = cm_transport.clone();
            return Ok(vec![
                TunnelAction::SpawnForward {
                    args,
                    transport: cm_transport,
                },
                TunnelAction::WaitForForward {
                    local_port: *local_port,
                },
            ]);
        }
        if !prompt_available || *auth_prompts >= 2 || is_ssh_agent_signing_failure(error_message) {
            let tail = stderr_tail.clone();
            self.state = SshTunnelState::Failed {
                message: error_message.to_owned(),
                stderr_tail: tail,
            };
            return Ok(Vec::new());
        }
        *auth_prompts += 1;
        Ok(vec![TunnelAction::PromptPassword {
            attempt: *auth_prompts,
        }])
    }

    fn on_forward_listening(&mut self) -> Result<Vec<TunnelAction>, SshError> {
        let SshTunnelState::StartingForward {
            key,
            target,
            local_port,
            remote_port,
            remote_server_kind,
            transport,
            ..
        } = &self.state
        else {
            return Err(invalid_transition(&self.state, "ForwardListening"));
        };
        let http_base_url = format!("http://127.0.0.1:{local_port}/");
        let ws_base_url = format!("ws://127.0.0.1:{local_port}/");
        let endpoints = TunnelEndpoints {
            key: key.clone(),
            target: target.clone(),
            local_port: *local_port,
            remote_port: *remote_port,
            http_base_url: http_base_url.clone(),
            ws_base_url,
            remote_server_kind: *remote_server_kind,
        };
        self.state = SshTunnelState::ProbingReadiness {
            endpoints,
            transport: transport.clone(),
        };
        Ok(vec![TunnelAction::ProbeHttpReady { http_base_url }])
    }

    fn on_http_ready(&mut self) -> Result<Vec<TunnelAction>, SshError> {
        let SshTunnelState::ProbingReadiness {
            endpoints,
            transport,
        } = &self.state
        else {
            return Err(invalid_transition(&self.state, "HttpReady"));
        };
        self.state = SshTunnelState::Ready {
            endpoints: endpoints.clone(),
            transport: transport.clone(),
        };
        Ok(Vec::new())
    }

    fn begin_close(&mut self, stop_remote: bool) -> Result<Vec<TunnelAction>, SshError> {
        let (endpoints, transport) = match &self.state {
            SshTunnelState::Ready {
                endpoints,
                transport,
            }
            | SshTunnelState::ProbingReadiness {
                endpoints,
                transport,
            } => (endpoints.clone(), transport.clone()),
            _ => return Err(invalid_transition(&self.state, "Close")),
        };
        let control_cancel_args = match transport {
            TunnelTransport::Direct => None,
            TunnelTransport::ControlMaster { control_path } => {
                Some(build_control_master_cancel_args(
                    &endpoints.target,
                    &control_path,
                    endpoints.local_port,
                    endpoints.remote_port,
                )?)
            }
        };
        self.state = SshTunnelState::Closing {
            key: endpoints.key,
            stop_remote,
        };
        Ok(vec![TunnelAction::CloseForward {
            control_cancel_args,
            stop_remote,
        }])
    }

    fn on_process_exited(&mut self, exit_code: Option<i32>) -> Result<Vec<TunnelAction>, SshError> {
        let (alias, stderr_tail) = match &self.state {
            SshTunnelState::StartingForward {
                target,
                stderr_tail,
                ..
            } => (target.alias.clone(), stderr_tail.clone()),
            SshTunnelState::ProbingReadiness { endpoints, .. }
            | SshTunnelState::Ready { endpoints, .. } => {
                (endpoints.target.alias.clone(), String::new())
            }
            _ => return Err(invalid_transition(&self.state, "ProcessExited")),
        };
        let trimmed = stderr_tail.trim();
        let message = if trimmed.is_empty() {
            format!(
                "SSH tunnel exited unexpectedly for {alias} (exit {}).",
                exit_code.map_or_else(|| "unknown".to_owned(), |c| c.to_string())
            )
        } else {
            trimmed.to_owned()
        };
        self.state = SshTunnelState::Failed {
            message,
            stderr_tail,
        };
        Ok(Vec::new())
    }

    fn on_remote_shutting_down(&mut self, pid: u32) -> Result<Vec<TunnelAction>, SshError> {
        match &self.state {
            SshTunnelState::Resolving { .. }
            | SshTunnelState::LaunchingRemote { .. }
            | SshTunnelState::StartingForward { .. }
            | SshTunnelState::ProbingReadiness { .. }
            | SshTunnelState::Ready { .. } => {
                let short = t3_contracts::brand::ACTIVE_BRAND
                    .cli_bin_name
                    .to_ascii_uppercase();
                self.state = SshTunnelState::Failed {
                    message: format!(
                        "The previous remote {short} server (PID {pid}) is still shutting down. Retry once it has exited."
                    ),
                    stderr_tail: String::new(),
                };
                Ok(Vec::new())
            }
            _ => Err(invalid_transition(&self.state, "RemoteShuttingDown")),
        }
    }
}

/// Appends `chunk` to `tail`, retaining at most [`TUNNEL_STDERR_TAIL_CHARS`] UTF-8 characters.
pub fn append_stderr_tail(tail: &mut String, chunk: &str) {
    tail.push_str(chunk);
    let char_count = tail.chars().count();
    if char_count > TUNNEL_STDERR_TAIL_CHARS {
        let skip = char_count - TUNNEL_STDERR_TAIL_CHARS;
        if let Some((byte_idx, _)) = tail.char_indices().nth(skip) {
            tail.drain(..byte_idx);
        }
    }
}

fn state_name(state: &SshTunnelState) -> &'static str {
    match state {
        SshTunnelState::Idle => "Idle",
        SshTunnelState::Resolving { .. } => "Resolving",
        SshTunnelState::LaunchingRemote { .. } => "LaunchingRemote",
        SshTunnelState::StartingForward { .. } => "StartingForward",
        SshTunnelState::ProbingReadiness { .. } => "ProbingReadiness",
        SshTunnelState::Ready { .. } => "Ready",
        SshTunnelState::Closing { .. } => "Closing",
        SshTunnelState::Closed => "Closed",
        SshTunnelState::Failed { .. } => "Failed",
    }
}

fn invalid_transition(state: &SshTunnelState, event: &'static str) -> SshError {
    SshError::InvalidStateTransition {
        from: state_name(state),
        event,
    }
}
