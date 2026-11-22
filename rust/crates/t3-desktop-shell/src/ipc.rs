//! Typed Tauri v2 IPC JSON envelope router combining [`t3_surfaces`], [`t3_edge`],
//! [`t3_persistence`], and [`crate::sidecar::SidecarController`].

use std::path::PathBuf;

use serde_json::Value;
use sha2::{Digest, Sha256};
use t3_edge::boot_service_units::{
    BOOT_SERVICE_LOG_FILE, BOOT_SERVICE_PLIST_FILE, BOOT_SERVICE_TASK_FILE, BOOT_SERVICE_UNIT_FILE,
    BootServicePlan, encode_windows_task_utf16le_bom, render_boot_service_launchd_plist,
    render_boot_service_systemd_unit, render_boot_service_windows_task_xml,
};
use t3_edge::desktop_updates::DesktopUpdateMachine;
use t3_persistence::paths::ServerDerivedPaths;
use t3_persistence::service_preflight::{SERVICE_LAUNCHER_PROTOCOL, run_service_preflight};
use t3_surfaces::SURFACE_PROTOCOL_VERSION;
use t3_surfaces::desktop::{
    DESKTOP_DEVELOPMENT_SCHEME, DESKTOP_HOST, DESKTOP_PRODUCTION_SCHEME, DisplayRect, WindowBounds,
    build_desktop_csp,
};

use crate::apple_speech_ipc::{
    AppleSpeechHelper, AppleSpeechIpcService, UnavailableAppleSpeechHelper,
    handle_apple_speech_available, handle_apple_speech_prepare,
    handle_apple_speech_supported_locales, handle_apple_speech_transcribe,
};
use crate::ipc_handlers::{
    apply_update_action, decode_optional_params, decode_params, encode_result,
    handle_clamp_window_bounds, handle_resolve_scheme, infer_channel_from_version, ipc_error,
    resolve_branding,
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
        Self {
            sidecar,
            update_machine: DesktopUpdateMachine::new(channel, version),
            apple_speech: AppleSpeechIpcService::default(),
        }
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
            None => Err(ipc_error(
                "UnknownMethod",
                format!("unsupported desktop IPC method `{}`", request.method),
            )),
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

        let brand = t3_contracts::brand::ACTIVE_BRAND;
        let dev_scheme = format!("{}-dev", brand.url_scheme);
        let scheme = if is_development {
            if brand.url_scheme == "t3code" {
                DESKTOP_DEVELOPMENT_SCHEME
            } else {
                dev_scheme.as_str()
            }
        } else if brand.url_scheme == "t3code" {
            DESKTOP_PRODUCTION_SCHEME
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
        let req: RenderServiceUnitsParams = decode_optional_params(params)?;
        let base_dir = req.base_dir.unwrap_or_else(|| {
            self.sidecar
                .config()
                .base_dir
                .to_string_lossy()
                .into_owned()
        });
        let program = req
            .program
            .unwrap_or_else(|| vec!["/usr/bin/t3".to_owned(), "__service-launcher".to_owned()]);
        let log_path = req
            .log_path
            .unwrap_or_else(|| format!("{base_dir}/userdata/logs/{BOOT_SERVICE_LOG_FILE}"));
        let unit_path = req
            .unit_path
            .unwrap_or_else(|| format!("{base_dir}/services/{BOOT_SERVICE_UNIT_FILE}"));

        let mut plan = BootServicePlan::new(program, base_dir, log_path, unit_path);
        if let Some(bin_dir) = req.install_bin_dir {
            plan = plan.with_install_bin_dir(bin_dir);
        }

        let systemd_unit = render_boot_service_systemd_unit(&plan)
            .map_err(|err| ipc_error("BootServiceRenderError", err.to_string()))?;
        let launchd_plist = render_boot_service_launchd_plist(
            &plan,
            req.home_dir.as_deref().unwrap_or("/Users/t3"),
            req.environment_path
                .as_deref()
                .unwrap_or("/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin"),
        );
        let windows_task_xml = render_boot_service_windows_task_xml(
            &plan,
            req.windows_user_id.as_deref().unwrap_or("DESKTOP\\t3"),
            req.home_dir.as_deref().unwrap_or("C:\\Users\\t3"),
            req.conhost_path
                .as_deref()
                .unwrap_or("C:\\Windows\\System32\\conhost.exe"),
        );
        let bom_bytes = encode_windows_task_utf16le_bom(&windows_task_xml);
        let sha256_hex = hex::encode(Sha256::digest(&bom_bytes));

        encode_result(RenderServiceUnitsResult {
            systemd_unit,
            launchd_plist,
            windows_task_xml,
            windows_task_utf16le_bom_sha256: sha256_hex,
            windows_task_utf16le_bom_bytes: bom_bytes.len(),
            file_names: ServiceUnitFileNames {
                systemd: BOOT_SERVICE_UNIT_FILE.to_owned(),
                launchd: BOOT_SERVICE_PLIST_FILE.to_owned(),
                windows_task: BOOT_SERVICE_TASK_FILE.to_owned(),
            },
        })
    }
}
