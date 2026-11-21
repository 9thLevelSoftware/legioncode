//! Pure helper handlers for [`crate::ipc::DesktopIpcRouter`].

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use t3_edge::desktop_updates::{
    DesktopUpdateChannel, DesktopUpdateMachine, PendingUpdate, VerifiedArtifact, verify_sha512,
};
use t3_surfaces::desktop::{
    DESKTOP_DEVELOPMENT_SCHEME, DESKTOP_PRODUCTION_SCHEME, DesktopSchemeRouter, WindowBounds,
};

use crate::ipc_types::{
    ClampWindowBoundsParams, ClampWindowBoundsResult, DesktopAppBranding, DesktopIpcErrorPayload,
    ResolveSchemeParams, ResolveSchemeResult,
};
use crate::ipc_update_types::UpdateStepAction;

pub(crate) fn handle_resolve_scheme(params: Value) -> Result<Value, DesktopIpcErrorPayload> {
    let req: ResolveSchemeParams = decode_params(params)?;
    let scheme = req.scheme.unwrap_or_else(|| {
        if req.is_development.unwrap_or(false) {
            DESKTOP_DEVELOPMENT_SCHEME.to_owned()
        } else {
            DESKTOP_PRODUCTION_SCHEME.to_owned()
        }
    });
    let mut router = DesktopSchemeRouter::new(scheme, req.clerk_frontend_api_hostname.as_deref());
    if let Some(assets) = req.assets {
        router = router.with_assets(assets);
    }
    let method = req.method.as_deref().unwrap_or("GET");
    let decision = router.route(method, &req.url, req.accept_header.as_deref());
    let status = decision.status();
    encode_result(ResolveSchemeResult {
        status,
        allowed: status == 200,
        decision,
    })
}

pub(crate) fn handle_clamp_window_bounds(params: Value) -> Result<Value, DesktopIpcErrorPayload> {
    let req: ClampWindowBoundsParams = decode_params(params)?;
    let Some(target_bounds) = req.bounds.or(req.persisted_bounds) else {
        return Err(ipc_error(
            "InvalidParams",
            "provide `bounds` or `persistedBounds` for desktop.clampWindowBounds",
        ));
    };
    let clamped = target_bounds.sanitize_clamped(&req.displays);
    let initial = WindowBounds::resolve_initial(req.persisted_bounds.or(req.bounds), &req.displays);
    let fits_within_any_display = req
        .displays
        .iter()
        .any(|display| target_bounds.fits_within_display(*display));
    let meets_minimum_size = target_bounds.meets_minimum_size();
    let normal_on_window_display = req
        .normal_bounds
        .map(|normal| normal.normal_on_window_display(target_bounds, &req.displays));

    encode_result(ClampWindowBoundsResult {
        clamped,
        initial,
        fits_within_any_display,
        meets_minimum_size,
        normal_on_window_display,
    })
}

pub(crate) fn apply_update_action(
    machine: &mut DesktopUpdateMachine,
    action: UpdateStepAction,
) -> Result<(), DesktopIpcErrorPayload> {
    match action {
        UpdateStepAction::GetState => {}
        UpdateStepAction::Reset {
            channel,
            current_version,
        } => {
            let next_channel = channel.unwrap_or_else(|| machine.channel());
            let next_version =
                current_version.unwrap_or_else(|| machine.current_version().to_owned());
            *machine = DesktopUpdateMachine::new(next_channel, next_version);
        }
        UpdateStepAction::ApplyManagedPolicy { policy } => {
            machine.apply_managed_policy(policy.as_ref());
        }
        UpdateStepAction::BeginCheck { checked_at } => machine.begin_check(checked_at),
        UpdateStepAction::UpdateAvailable {
            version,
            checked_at,
            release_notes,
        } => {
            let mut pending = PendingUpdate::new(version, checked_at);
            pending.release_notes = release_notes;
            machine.on_update_available(pending);
        }
        UpdateStepAction::NoUpdate { checked_at } => machine.on_no_update(checked_at),
        UpdateStepAction::CheckFailure {
            message,
            checked_at,
        } => machine.on_check_failure(&message, checked_at),
        UpdateStepAction::BeginDownload => machine
            .begin_download()
            .map_err(|err| ipc_error("UpdateStateError", err.to_string()))?,
        UpdateStepAction::DownloadProgress { percent } => machine
            .on_download_progress(percent)
            .map_err(|err| ipc_error("UpdateStateError", err.to_string()))?,
        UpdateStepAction::DownloadFailure { message } => machine.on_download_failure(&message),
        UpdateStepAction::DownloadComplete {
            version,
            sha512,
            size_bytes,
            payload_utf8,
        } => {
            if let Some(payload) = payload_utf8.as_deref() {
                verify_sha512(payload.as_bytes(), &sha512)
                    .map_err(|err| ipc_error("DigestVerificationError", err.to_string()))?;
            }
            machine.on_download_complete(VerifiedArtifact::new(version, sha512, size_bytes));
        }
        UpdateStepAction::InstallFailure { message } => machine.on_install_failure(&message),
    }
    Ok(())
}

pub(crate) fn resolve_branding(is_development: bool, app_version: &str) -> DesktopAppBranding {
    let stage = if is_development {
        "Dev"
    } else if app_version.to_ascii_lowercase().contains("nightly") {
        "Nightly"
    } else {
        "Beta"
    };
    DesktopAppBranding {
        base_name: "T3 Code".to_owned(),
        stage_label: stage.to_owned(),
        display_name: format!("T3 Code ({stage})"),
    }
}

pub(crate) fn infer_channel_from_version(app_version: &str) -> DesktopUpdateChannel {
    if app_version.to_ascii_lowercase().contains("nightly") {
        DesktopUpdateChannel::Nightly
    } else {
        DesktopUpdateChannel::Latest
    }
}

pub(crate) fn decode_params<T: DeserializeOwned>(
    params: Value,
) -> Result<T, DesktopIpcErrorPayload> {
    serde_json::from_value(params).map_err(|err| ipc_error("InvalidParams", err.to_string()))
}

pub(crate) fn decode_optional_params<T: Default + DeserializeOwned>(
    params: Value,
) -> Result<T, DesktopIpcErrorPayload> {
    if params.is_null() {
        Ok(T::default())
    } else {
        decode_params(params)
    }
}

pub(crate) fn encode_result<T: Serialize>(value: T) -> Result<Value, DesktopIpcErrorPayload> {
    serde_json::to_value(value).map_err(|err| ipc_error("SerializationError", err.to_string()))
}

pub(crate) fn ipc_error(code: &str, message: impl Into<String>) -> DesktopIpcErrorPayload {
    DesktopIpcErrorPayload {
        code: code.to_owned(),
        message: message.into(),
    }
}
