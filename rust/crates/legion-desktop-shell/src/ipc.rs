//! Typed Tauri v2 IPC JSON envelope router combining [`legion_surfaces`], [`legion_edge`],
//! [`legion_persistence`], and [`crate::sidecar::SidecarController`].

use std::path::PathBuf;

use legion_edge::desktop_updates::DesktopUpdateMachine;
use legion_persistence::paths::ServerDerivedPaths;
use legion_persistence::service_preflight::{SERVICE_LAUNCHER_PROTOCOL, run_service_preflight};
use legion_surfaces::SURFACE_PROTOCOL_VERSION;
use legion_surfaces::desktop::{DESKTOP_HOST, DisplayRect, WindowBounds, build_desktop_csp};
use serde_json::Value;

use crate::apple_speech_ipc::{
    AppleSpeechHelper, AppleSpeechIpcService, UnavailableAppleSpeechHelper,
    handle_apple_speech_available, handle_apple_speech_prepare,
    handle_apple_speech_supported_locales, handle_apple_speech_transcribe,
};
use crate::bridge_types::DesktopBridgeRuntimeState;
use crate::ipc_bridge::dispatch_core_bridge;
use crate::ipc_bridge_extra::dispatch_extra_bridge;
use crate::ipc_handlers::{
    apply_update_action, decode_optional_params, decode_params, encode_result,
    handle_clamp_window_bounds, handle_render_units, handle_resolve_scheme,
    infer_channel_from_version, ipc_error, resolve_branding,
};
pub use crate::ipc_types::{
    BootstrapParams, BootstrapResult, ClampWindowBoundsParams, ClampWindowBoundsResult,
    DesktopAppBranding, DesktopIpcErrorPayload, DesktopIpcMethod, DesktopIpcRequest,
    DesktopIpcResponse, ResolveSchemeParams, ResolveSchemeResult,
};
pub use crate::ipc_update_types::{
    PreflightDbParams, PreflightDbResult, RenderServiceUnitsParams, RenderServiceUnitsResult,
    ServiceUnitFileNames, UpdateStepAction, UpdateStepParams, UpdateStepResult,
};
use crate::sidecar::{SidecarConfig, SidecarController, SidecarMode};

/// Stateful Tauri v2 IPC router dispatching typed JSON envelopes to Rust domain cores.
#[derive(Debug, Clone)]
pub struct DesktopIpcRouter {
    sidecar: SidecarController,
    update_machine: DesktopUpdateMachine,
    apple_speech: AppleSpeechIpcService,
    bridge_state: DesktopBridgeRuntimeState,
}

impl Default for DesktopIpcRouter {
    fn default() -> Self {
        Self::new(SidecarController::default())
    }
}

impl DesktopIpcRouter {
    /// Creates a new [`DesktopIpcRouter`] backed by `sidecar`.
    pub fn new(sidecar: SidecarController) -> Self {
        let version = sidecar.config().app_version.clone();
        let channel = infer_channel_from_version(&version);
        let bridge_state = DesktopBridgeRuntimeState::from_base_dir(&sidecar.config().base_dir);
        Self {
            sidecar,
            update_machine: DesktopUpdateMachine::new(channel, version),
            apple_speech: AppleSpeechIpcService::default(),
            bridge_state,
        }
    }

    /// Configures local environment URLs and pairing bootstrap token for `window.desktopBridge`.
    #[must_use]
    pub fn with_local_bootstrap(
        mut self,
        http_base_url: Option<String>,
        ws_base_url: Option<String>,
        bootstrap_token: Option<String>,
    ) -> Self {
        self.bridge_state.local_http_base_url = http_base_url;
        self.bridge_state.local_ws_base_url = ws_base_url;
        self.bridge_state.local_bootstrap_token = bootstrap_token;
        self
    }

    /// Borrows the runtime state backing `window.desktopBridge`.
    pub const fn bridge_state(&self) -> &DesktopBridgeRuntimeState {
        &self.bridge_state
    }

    /// Mutably borrows the runtime state backing `window.desktopBridge`.
    pub fn bridge_state_mut(&mut self) -> &mut DesktopBridgeRuntimeState {
        &mut self.bridge_state
    }

    /// Resolves current [`DesktopAppBranding`] metadata matching `DesktopEnvironment.ts`.
    #[must_use]
    pub fn resolve_app_branding(&self) -> DesktopAppBranding {
        resolve_branding(
            self.sidecar.config().mode.is_development(),
            self.update_machine.current_version(),
        )
    }

    /// Replaces the initial [`DesktopUpdateMachine`] carried by this router.
    #[must_use]
    pub fn with_update_machine(mut self, update_machine: DesktopUpdateMachine) -> Self {
        self.update_machine = update_machine;
        self
    }

    /// Configures Apple Speech capabilities for `desktop.appleSpeech.*` IPC calls (`#34027`).
    #[must_use]
    pub fn with_apple_speech(mut self, apple_speech: AppleSpeechIpcService) -> Self {
        self.apple_speech = apple_speech;
        self
    }

    /// Borrow mutable [`AppleSpeechIpcService`] state.
    pub fn apple_speech_mut(&mut self) -> &mut AppleSpeechIpcService {
        &mut self.apple_speech
    }

    /// Returns the configured [`SidecarController`].
    pub const fn sidecar(&self) -> &SidecarController {
        &self.sidecar
    }

    /// Returns the router's current [`DesktopUpdateMachine`] state.
    pub const fn update_machine(&self) -> &DesktopUpdateMachine {
        &self.update_machine
    }

    /// Dispatches a decoded [`DesktopIpcRequest`] using default helper availability.
    pub fn dispatch(&mut self, request: DesktopIpcRequest) -> DesktopIpcResponse {
        let mut unavailable = UnavailableAppleSpeechHelper;
        self.dispatch_with_apple_speech(request, &mut unavailable)
    }

    /// Dispatches a decoded [`DesktopIpcRequest`] with a supplied [`AppleSpeechHelper`] backend (`#34027`).
    pub fn dispatch_with_apple_speech<H: AppleSpeechHelper>(
        &mut self,
        request: DesktopIpcRequest,
        speech_helper: &mut H,
    ) -> DesktopIpcResponse {
        let id = request.id;
        let outcome = match DesktopIpcMethod::parse(&request.method) {
            Some(DesktopIpcMethod::Bootstrap) => self.handle_bootstrap(request.params),
            Some(DesktopIpcMethod::ResolveScheme) => handle_resolve_scheme(request.params),
            Some(DesktopIpcMethod::ClampWindowBounds) => handle_clamp_window_bounds(request.params),
            Some(DesktopIpcMethod::UpdateStep) => self.handle_update_step(request.params),
            Some(DesktopIpcMethod::PreflightDb) => self.handle_preflight_db(request.params),
            Some(DesktopIpcMethod::RenderServiceUnits) => self.handle_render_units(request.params),
            Some(DesktopIpcMethod::AppleSpeechAvailable) => {
                handle_apple_speech_available(&self.apple_speech)
            }
            Some(DesktopIpcMethod::AppleSpeechSupportedLocales) => {
                handle_apple_speech_supported_locales(&self.apple_speech)
            }
            Some(DesktopIpcMethod::AppleSpeechPrepare) => {
                handle_apple_speech_prepare(&mut self.apple_speech, &request.params, speech_helper)
            }
            Some(DesktopIpcMethod::AppleSpeechTranscribe) => handle_apple_speech_transcribe(
                &mut self.apple_speech,
                request.params,
                speech_helper,
            ),
            None => {
                if request.method.starts_with("desktop.bridge.") {
                    if let Some(outcome) = dispatch_core_bridge(
                        &request.method,
                        request.params.clone(),
                        &mut self.bridge_state,
                        &mut self.update_machine,
                    )
                    .or_else(|| {
                        dispatch_extra_bridge(
                            &request.method,
                            request.params,
                            &mut self.bridge_state,
                            &self.sidecar,
                        )
                    }) {
                        outcome
                    } else {
                        Err(ipc_error(
                            "UnknownMethod",
                            format!("unsupported desktop bridge method `{}`", request.method),
                        ))
                    }
                } else {
                    Err(ipc_error(
                        "UnknownMethod",
                        format!("unsupported desktop IPC method `{}`", request.method),
                    ))
                }
            }
        };

        match outcome {
            Ok(result) => DesktopIpcResponse {
                id,
                ok: true,
                result: Some(result),
                error: None,
            },
            Err(error) => DesktopIpcResponse {
                id,
                ok: false,
                result: None,
                error: Some(error),
            },
        }
    }

    /// Parses `raw_json` into a [`DesktopIpcRequest`] and dispatches it.
    pub fn dispatch_json(
        &mut self,
        raw_json: &str,
    ) -> Result<DesktopIpcResponse, serde_json::Error> {
        let request: DesktopIpcRequest = serde_json::from_str(raw_json)?;
        Ok(self.dispatch(request))
    }

    /// Evaluates a raw JSON IPC envelope and returns the compact JSON response string.
    pub fn eval_json_string(&mut self, raw_json: &str) -> Result<String, serde_json::Error> {
        let response = self.dispatch_json(raw_json)?;
        serde_json::to_string(&response)
    }

    fn handle_bootstrap(&self, params: Value) -> Result<Value, DesktopIpcErrorPayload> {
        let req: BootstrapParams = decode_optional_params(params)?;
        let cfg = self.sidecar.config();
        let app_version = req.app_version.unwrap_or_else(|| cfg.app_version.clone());
        let is_development = req
            .is_development
            .unwrap_or_else(|| cfg.mode.is_development());
        let base_dir = req
            .base_dir
            .map_or_else(|| cfg.base_dir.clone(), PathBuf::from);
        let base_dir_is_explicit = req.base_dir_is_explicit.unwrap_or(cfg.base_dir_is_explicit);
        let launcher_protocol = req.launcher_protocol.unwrap_or(cfg.launcher_protocol);

        let paths = ServerDerivedPaths::derive(&base_dir, is_development, base_dir_is_explicit);
        let preflight = run_service_preflight(&paths.db_path, launcher_protocol, &app_version);

        let brand = legion_contracts::brand::ACTIVE_BRAND;
        let dev_scheme = format!("{}-dev", brand.url_scheme);
        let scheme = if is_development {
            dev_scheme.as_str()
        } else {
            brand.url_scheme
        };
        let identifier = if is_development {
            format!("{}.dev", brand.macos_policy_domain)
        } else {
            brand.macos_policy_domain.to_owned()
        };
        let content_security_policy =
            build_desktop_csp(scheme, req.clerk_frontend_api_hostname.as_deref());
        let displays = req.displays.unwrap_or_else(|| {
            vec![DisplayRect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            }]
        });
        let initial_window_bounds = WindowBounds::resolve_initial(req.persisted_bounds, &displays);
        let clamped_window_bounds = req
            .persisted_bounds
            .map(|bounds| bounds.sanitize_clamped(&displays));

        let channel = req
            .update_channel
            .unwrap_or_else(|| infer_channel_from_version(&app_version));
        let mut machine = DesktopUpdateMachine::new(channel, &app_version);
        let updates_policy = req
            .managed_policy
            .as_ref()
            .and_then(|doc| doc.limits.as_ref())
            .and_then(|limits| limits.updates.as_ref());
        let updates_enabled = machine.apply_managed_policy(updates_policy);

        let sidecar = SidecarController::new(SidecarConfig {
            base_dir,
            mode: SidecarMode::from_is_development(is_development),
            base_dir_is_explicit,
            app_version: app_version.clone(),
            launcher_protocol,
            ..cfg.clone()
        });
        let discovery = sidecar.discover_runtime();
        let sidecar_launch_args = sidecar.launch_args();
        let sanitized_env = sidecar.sanitize_host_env();

        encode_result(BootstrapResult {
            surface_protocol_version: SURFACE_PROTOCOL_VERSION,
            identifier,
            scheme: scheme.to_owned(),
            desktop_url: format!("{scheme}://{DESKTOP_HOST}/"),
            content_security_policy,
            branding: resolve_branding(is_development, &app_version),
            initial_window_bounds,
            clamped_window_bounds,
            paths,
            preflight,
            updates_enabled,
            update_status: machine.status(),
            update_channel: machine.channel(),
            track_mismatch: machine.track_mismatch(),
            sidecar_binary: discovery
                .found
                .then(|| discovery.binary_path.to_string_lossy().into_owned()),
            sidecar_launch_args,
            sidecar_sanitized_env_count: sanitized_env.as_map().len(),
        })
    }

    fn handle_update_step(&mut self, params: Value) -> Result<Value, DesktopIpcErrorPayload> {
        let req: UpdateStepParams = decode_params(params)?;
        let using_external_state = req.state.is_some();
        let mut machine = req.state.unwrap_or_else(|| self.update_machine.clone());
        apply_update_action(&mut machine, req.action)?;
        if !using_external_state {
            self.update_machine = machine.clone();
        }
        encode_result(UpdateStepResult {
            status: machine.status(),
            channel: machine.channel(),
            current_version: machine.current_version().to_owned(),
            retained_download: machine.retained_download().cloned(),
            pending_check: machine.pending_check().cloned(),
            installable_artifact: machine.installable_artifact().cloned(),
            last_error: machine.last_error().map(str::to_owned),
            error_context: machine.error_context(),
            track_mismatch: machine.track_mismatch(),
            state: machine,
        })
    }

    fn handle_preflight_db(&self, params: Value) -> Result<Value, DesktopIpcErrorPayload> {
        let req: PreflightDbParams = decode_optional_params(params)?;
        let cfg = self.sidecar.config();
        let database_path = if let Some(explicit) = req.database_path {
            PathBuf::from(explicit)
        } else {
            let base_dir = req
                .base_dir
                .map_or_else(|| cfg.base_dir.clone(), PathBuf::from);
            let is_dev = req
                .is_development
                .unwrap_or_else(|| cfg.mode.is_development());
            let explicit_base = req.base_dir_is_explicit.unwrap_or(cfg.base_dir_is_explicit);
            ServerDerivedPaths::derive(base_dir, is_dev, explicit_base).db_path
        };
        let protocol = req.launcher_protocol.unwrap_or(SERVICE_LAUNCHER_PROTOCOL);
        let version = req.version.unwrap_or_else(|| cfg.app_version.clone());
        let preflight = run_service_preflight(&database_path, protocol, &version);

        encode_result(PreflightDbResult {
            database_path: database_path.to_string_lossy().into_owned(),
            ready: preflight.is_ready(),
            preflight,
        })
    }

    fn handle_render_units(&self, params: Value) -> Result<Value, DesktopIpcErrorPayload> {
        let default_base_dir = self.sidecar.config().base_dir.to_string_lossy();
        handle_render_units(&default_base_dir, params)
    }
}
