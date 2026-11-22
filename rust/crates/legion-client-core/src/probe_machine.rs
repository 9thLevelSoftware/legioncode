//! Deterministic state machine driving connected-lease probes and silent-socket checks.

use crate::probe::{
    ActiveProbe, ConnectionRouteKind, FRAME_CHECK_EVERY_MS, MonitorState, ProbeAction,
    ProbeEndReason, ProbeSignal, QUICK_CONNECTION_PROBE_TIMEOUT_MS, REMOTE_SILENT_SOCKET_AFTER_MS,
    ResumeMode, SILENT_SOCKET_CHECK_AFTER_MS, SILENT_SOCKET_RECHECK_EVERY_MS, SilentCheckPhase,
    is_foreground_probe, is_long_resume, probe_timeout_for,
};

/// Deterministic state machine for connected-lease probing and dead/silent-socket detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SilentSocketStateMachine {
    route_kind: Option<ConnectionRouteKind>,
    active: ActiveProbe,
    silent_check: SilentCheckPhase,
}

/// Canonical glossary alias for [`SilentSocketStateMachine`].
pub type SilentSocketMachine = SilentSocketStateMachine;

impl SilentSocketStateMachine {
    /// Creates an idle [`SilentSocketStateMachine`] for a connected lease on `route_kind`.
    pub const fn new(route_kind: Option<ConnectionRouteKind>) -> Self {
        Self {
            route_kind,
            active: ActiveProbe::Idle,
            silent_check: SilentCheckPhase::Idle,
        }
    }

    /// Returns `true` while a liveness probe cycle is active.
    pub const fn is_probing(&self) -> bool {
        matches!(self.active, ActiveProbe::Running { .. })
    }

    /// Returns the earliest timestamp in milliseconds when [`Self::on_tick`] needs to run.
    pub fn next_deadline_ms(&self) -> Option<u64> {
        let ActiveProbe::Running { deadline_ms, .. } = self.active else {
            return None;
        };
        let mut earliest = deadline_ms;
        match self.silent_check {
            SilentCheckPhase::Idle
            | SilentCheckPhase::AwaitingConfig { .. }
            | SilentCheckPhase::ConfigUnavailable => {}
            SilentCheckPhase::WaitingInitialCheck { due_at_ms, .. } => {
                earliest = earliest.min(due_at_ms);
            }
            SilentCheckPhase::Monitoring(m) => {
                earliest = earliest.min(m.next_frame_poll_ms);
                if let Some(host_at) = m.next_host_check_ms {
                    earliest = earliest.min(host_at);
                }
                if let Some(floor_at) = m.remote_floor_at_ms {
                    earliest = earliest.min(floor_at);
                }
            }
        }
        Some(earliest)
    }

    /// Processes a supervisor signal at `now_ms`.
    pub fn on_signal(&mut self, signal: ProbeSignal, now_ms: u64) -> Vec<ProbeAction> {
        let Some(timeout_ms) = probe_timeout_for(signal, self.route_kind) else {
            return Vec::new();
        };
        let foreground = is_foreground_probe(signal);
        let long = is_long_resume(signal);
        match &mut self.active {
            ActiveProbe::Idle => {
                let resume = if long {
                    ResumeMode::Long {
                        started_at_ms: now_ms,
                    }
                } else {
                    ResumeMode::Standard
                };
                self.active = ActiveProbe::Running {
                    probe_timeout_ms: timeout_ms,
                    deadline_ms: now_ms.saturating_add(timeout_ms),
                    tolerates_miss: foreground,
                    resume,
                    recheck_queued: false,
                };
                self.arm_silent_check(signal, now_ms);
                vec![ProbeAction::StartProbe]
            }
            ActiveProbe::Running {
                probe_timeout_ms,
                deadline_ms,
                tolerates_miss,
                resume,
                ..
            } => {
                if !foreground {
                    *tolerates_miss = false;
                }
                if long && matches!(resume, ResumeMode::Standard) {
                    *resume = ResumeMode::Long {
                        started_at_ms: now_ms,
                    };
                }
                let signal_deadline = now_ms.saturating_add(timeout_ms);
                if signal_deadline < *deadline_ms {
                    *deadline_ms = signal_deadline;
                    *probe_timeout_ms = timeout_ms;
                }
                self.arm_silent_check(signal, now_ms);
                Vec::new()
            }
        }
    }

    /// Reports the result of `LoadInitialConfig`.
    pub fn on_config_loaded(
        &mut self,
        loaded: bool,
        current_frames: u64,
        now_ms: u64,
    ) -> Vec<ProbeAction> {
        let SilentCheckPhase::AwaitingConfig { started_at_ms } = self.silent_check else {
            return Vec::new();
        };
        if !loaded {
            self.silent_check = SilentCheckPhase::ConfigUnavailable;
            return Vec::new();
        }
        self.silent_check = SilentCheckPhase::Monitoring(MonitorState {
            started_at_ms,
            frames_before: current_frames,
            next_frame_poll_ms: now_ms.saturating_add(FRAME_CHECK_EVERY_MS),
            next_host_check_ms: None,
            host_check_in_flight: true,
            remote_floor_at_ms: None,
        });
        vec![ProbeAction::CheckHost]
    }

    /// Reports an observed inbound frame count immediately (outside the `100` ms poll tick).
    pub fn on_frames_observed(&mut self, current_frames: u64, now_ms: u64) {
        if let SilentCheckPhase::Monitoring(m) = self.silent_check
            && current_frames != m.frames_before
        {
            self.resolve_socket_alive(now_ms);
        }
    }

    /// Reports whether an out-of-band `CheckHost` request answered at `now_ms`.
    pub fn on_host_check_result(
        &mut self,
        answered: bool,
        current_frames: u64,
        now_ms: u64,
    ) -> Vec<ProbeAction> {
        let SilentCheckPhase::Monitoring(mut m) = self.silent_check else {
            return Vec::new();
        };
        if current_frames != m.frames_before {
            self.resolve_socket_alive(now_ms);
            return Vec::new();
        }
        m.host_check_in_flight = false;
        if !answered {
            m.next_host_check_ms = Some(now_ms.saturating_add(SILENT_SOCKET_RECHECK_EVERY_MS));
            self.silent_check = SilentCheckPhase::Monitoring(m);
            return Vec::new();
        }
        if self.answers_quickly() || self.is_long_resume_active() {
            return self.finish_replace(ProbeEndReason::SilentSocket);
        }
        let floor_at = m
            .started_at_ms
            .saturating_add(REMOTE_SILENT_SOCKET_AFTER_MS);
        if now_ms >= floor_at {
            return self.finish_replace(ProbeEndReason::SilentSocket);
        }
        m.next_host_check_ms = None;
        m.remote_floor_at_ms = Some(floor_at);
        self.silent_check = SilentCheckPhase::Monitoring(m);
        Vec::new()
    }

    /// Reports completion of the RPC `probe` call.
    pub fn on_probe_completed(&mut self, succeeded: bool) -> Vec<ProbeAction> {
        if !self.is_probing() {
            return Vec::new();
        }
        self.reset_idle();
        vec![if succeeded {
            ProbeAction::ProbeHealthy
        } else {
            ProbeAction::ProbeFailed
        }]
    }

    /// Advances timers up to `now_ms`, using `current_frames` for any due frame poll.
    pub fn on_tick(&mut self, now_ms: u64, current_frames: u64) -> Vec<ProbeAction> {
        if !self.is_probing() {
            return Vec::new();
        }
        let mut actions = self.step_silent_check(now_ms, current_frames);
        let ActiveProbe::Running {
            probe_timeout_ms,
            deadline_ms,
            tolerates_miss,
            resume,
            ..
        } = &mut self.active
        else {
            return actions;
        };
        if now_ms >= *deadline_ms {
            if *tolerates_miss {
                *tolerates_miss = false;
                *deadline_ms = now_ms.saturating_add(*probe_timeout_ms);
                actions.push(ProbeAction::StartProbe);
            } else if matches!(resume, ResumeMode::Long { .. }) {
                actions.extend(self.finish_replace(ProbeEndReason::LongResumeTimeout));
            } else {
                self.reset_idle();
                actions.push(ProbeAction::ProbeTimedOut);
            }
        }
        actions
    }

    fn step_silent_check(&mut self, now_ms: u64, current_frames: u64) -> Vec<ProbeAction> {
        let mut actions = Vec::new();
        match self.silent_check {
            SilentCheckPhase::WaitingInitialCheck {
                started_at_ms,
                due_at_ms,
            } if now_ms >= due_at_ms => {
                self.silent_check = SilentCheckPhase::AwaitingConfig { started_at_ms };
                actions.push(ProbeAction::LoadInitialConfig);
            }
            SilentCheckPhase::Monitoring(mut m) => {
                let poll_due = now_ms >= m.next_frame_poll_ms;
                let floor_due = m.remote_floor_at_ms.is_some_and(|floor| now_ms >= floor);
                if (poll_due || floor_due) && current_frames != m.frames_before {
                    self.resolve_socket_alive(now_ms);
                } else if floor_due {
                    return self.finish_replace(ProbeEndReason::SilentSocket);
                } else {
                    if poll_due {
                        m.next_frame_poll_ms = now_ms.saturating_add(FRAME_CHECK_EVERY_MS);
                        actions.push(ProbeAction::PollInboundFrames);
                    }
                    if let Some(host_at) = m.next_host_check_ms
                        && !m.host_check_in_flight
                        && now_ms >= host_at
                    {
                        m.next_host_check_ms = None;
                        m.host_check_in_flight = true;
                        actions.push(ProbeAction::CheckHost);
                    }
                    self.silent_check = SilentCheckPhase::Monitoring(m);
                }
            }
            _ => {}
        }
        actions
    }

    fn answers_quickly(&self) -> bool {
        matches!(self.route_kind, Some(kind) if kind.answers_quickly())
    }

    const fn is_long_resume_active(&self) -> bool {
        matches!(
            self.active,
            ActiveProbe::Running {
                resume: ResumeMode::Long { .. },
                ..
            }
        )
    }

    fn arm_silent_check(&mut self, signal: ProbeSignal, now_ms: u64) {
        if is_foreground_probe(signal) {
            return;
        }
        if !matches!(self.silent_check, SilentCheckPhase::Idle) {
            if let ActiveProbe::Running { recheck_queued, .. } = &mut self.active {
                *recheck_queued = true;
            }
            return;
        }
        self.start_silent_check(now_ms);
    }

    fn start_silent_check(&mut self, now_ms: u64) {
        if let ActiveProbe::Running { recheck_queued, .. } = &mut self.active {
            *recheck_queued = false;
        }
        self.silent_check = SilentCheckPhase::WaitingInitialCheck {
            started_at_ms: now_ms,
            due_at_ms: now_ms.saturating_add(SILENT_SOCKET_CHECK_AFTER_MS),
        };
    }

    fn resolve_socket_alive(&mut self, now_ms: u64) {
        self.silent_check = SilentCheckPhase::Idle;
        let should_recheck = if let ActiveProbe::Running {
            deadline_ms,
            resume: ResumeMode::Long { started_at_ms },
            recheck_queued,
            ..
        } = &mut self.active
        {
            let alive_deadline = started_at_ms.saturating_add(QUICK_CONNECTION_PROBE_TIMEOUT_MS);
            if alive_deadline > *deadline_ms {
                *deadline_ms = alive_deadline;
            }
            *recheck_queued
        } else if let ActiveProbe::Running { recheck_queued, .. } = self.active {
            recheck_queued
        } else {
            false
        };
        if should_recheck {
            self.start_silent_check(now_ms);
        }
    }

    fn finish_replace(&mut self, reason: ProbeEndReason) -> Vec<ProbeAction> {
        self.reset_idle();
        vec![ProbeAction::ReplaceSocket { reason }]
    }
    fn reset_idle(&mut self) {
        self.active = ActiveProbe::Idle;
        self.silent_check = SilentCheckPhase::Idle;
    }
}
