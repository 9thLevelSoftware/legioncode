//! Pure helper handlers for [`crate::ipc::DesktopIpcRouter`].

use std::time::{SystemTime, UNIX_EPOCH};

use legion_edge::boot_service_units::{
    BOOT_SERVICE_LOG_FILE, BOOT_SERVICE_PLIST_FILE, BOOT_SERVICE_TASK_FILE, BOOT_SERVICE_UNIT_FILE,
    BootServicePlan, encode_windows_task_utf16le_bom, render_boot_service_launchd_plist,
    render_boot_service_systemd_unit, render_boot_service_windows_task_xml,
};
use legion_edge::desktop_updates::{
    DesktopUpdateChannel, DesktopUpdateMachine, PendingUpdate, UpdateStatus, VerifiedArtifact,
    desktop_update_channel_of, verify_sha512,
};
use legion_surfaces::desktop::{
    DESKTOP_DEVELOPMENT_SCHEME, DESKTOP_PRODUCTION_SCHEME, DesktopSchemeRouter, WindowBounds,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::bridge_types::{
    DesktopUpdateReleaseNotePayload, DesktopUpdateStatePayload, current_runtime_arch,
};
use crate::ipc_types::{
    ClampWindowBoundsParams, ClampWindowBoundsResult, DesktopAppBranding, DesktopIpcErrorPayload,
    ResolveSchemeParams, ResolveSchemeResult,
};
use crate::ipc_update_types::{
    RenderServiceUnitsParams, RenderServiceUnitsResult, ServiceUnitFileNames, UpdateStepAction,
};

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
        UpdateStepAction::NoOffer {
            checked_at,
            has_release_floor,
            managed_policy,
        } => {
            machine.on_no_offer(checked_at, has_release_floor, managed_policy.as_ref());
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
    let product_name = legion_contracts::brand::ACTIVE_BRAND.product_name;
    DesktopAppBranding {
        base_name: product_name.to_owned(),
        stage_label: stage.to_owned(),
        display_name: format!("{product_name} ({stage})"),
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetUpdateChannelParams {
    channel: DesktopUpdateChannel,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct InstallUpdateParams {
    expected_version: Option<String>,
}

pub(crate) fn build_update_state(machine: &DesktopUpdateMachine) -> DesktopUpdateStatePayload {
    let serialized = serde_json::to_value(machine).ok();
    let checked_at = serialized
        .as_ref()
        .and_then(|v| v.get("checked_at"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let release_notes = machine
        .pending_check()
        .filter(|pending| !pending.release_notes.is_empty())
        .map_or_default(|pending| {
            vec![DesktopUpdateReleaseNotePayload {
                version: pending.version.clone(),
                total_items: pending.release_notes.len(),
                items: pending.release_notes.clone(),
            }]
        });
    let arch = current_runtime_arch().to_owned();
    DesktopUpdateStatePayload {
        enabled: machine.status() != UpdateStatus::Disabled,
        status: machine.status(),
        channel: machine.channel(),
        current_version: machine.current_version().to_owned(),
        host_arch: arch.clone(),
        app_arch: arch,
        running_under_arm64_translation: false,
        available_version: machine.pending_check().map(|p| p.version.clone()),
        downloaded_version: machine.retained_download().map(|d| d.version.clone()),
        release_notes,
        omitted_release_count: 0,
        download_percent: machine.pending_check().and_then(|p| p.download_percent),
        checked_at,
        message: machine.last_error().map(str::to_owned),
        error_context: machine.error_context(),
        can_retry: machine.status() == UpdateStatus::Error,
    }
}

pub(crate) fn handle_set_update_channel(
    params: Value,
    machine: &mut DesktopUpdateMachine,
) -> Result<Value, DesktopIpcErrorPayload> {
    let req: SetUpdateChannelParams =
        serde_json::from_value(params).map_err(|e| ipc_error("InvalidParams", e.to_string()))?;
    let version = machine.current_version().to_owned();
    let mut next = DesktopUpdateMachine::new(req.channel, version);
    if machine.status() == UpdateStatus::Disabled
        && req.channel == desktop_update_channel_of(next.current_version())
    {
        next.apply_managed_policy(None);
    }
    *machine = next;
    encode_result(build_update_state(machine))
}

pub(crate) fn handle_check_for_update(machine: &mut DesktopUpdateMachine) -> Value {
    if machine.status() == UpdateStatus::Disabled {
        return json!({ "checked": false, "state": build_update_state(machine) });
    }
    let timestamp = now_iso_seconds();
    machine.begin_check(&timestamp);
    machine.on_no_update(timestamp);
    json!({ "checked": true, "state": build_update_state(machine) })
}

pub(crate) fn handle_download_update(machine: &mut DesktopUpdateMachine) -> Value {
    let (accepted, completed) = match machine.begin_download() {
        Ok(()) => (true, false),
        Err(_) => (false, machine.retained_download().is_some()),
    };
    json!({
        "accepted": accepted,
        "completed": completed,
        "state": build_update_state(machine),
    })
}

pub(crate) fn handle_install_update(
    params: Value,
    machine: &mut DesktopUpdateMachine,
) -> Result<Value, DesktopIpcErrorPayload> {
    let req: InstallUpdateParams = decode_optional_params(params)?;
    let matching_download = machine.retained_download().is_some_and(|artifact| {
        req.expected_version
            .as_deref()
            .is_none_or(|expected| expected == artifact.version)
    });
    Ok(json!({
        "accepted": matching_download,
        "completed": false,
        "state": build_update_state(machine),
    }))
}

fn now_iso_seconds() -> String {
    let total_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0_i64, |d| i64::try_from(d.as_secs()).unwrap_or(0));
    let days = total_secs.div_euclid(86_400);
    let secs_of_day = total_secs.rem_euclid(86_400);
    let hour = secs_of_day / 3_600;
    let minute = (secs_of_day % 3_600) / 60;
    let second = secs_of_day % 60;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let adj_year = year + i64::from(month <= 2);
    format!("{adj_year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

pub(crate) fn handle_render_units(
    default_base_dir: &str,
    params: Value,
) -> Result<Value, DesktopIpcErrorPayload> {
    let req: RenderServiceUnitsParams = decode_optional_params(params)?;
    let base_dir = req.base_dir.unwrap_or_else(|| default_base_dir.to_owned());
    let program = req.program.unwrap_or_else(|| {
        vec![
            "/usr/bin/legioncode".to_owned(),
            "__service-launcher".to_owned(),
        ]
    });
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
        req.home_dir.as_deref().unwrap_or("/Users/legioncode"),
        req.environment_path
            .as_deref()
            .unwrap_or("/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin"),
    );
    let windows_task_xml = render_boot_service_windows_task_xml(
        &plan,
        req.windows_user_id
            .as_deref()
            .unwrap_or("DESKTOP\\legioncode"),
        req.home_dir.as_deref().unwrap_or("C:\\Users\\legioncode"),
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
