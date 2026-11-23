//! `window.desktopBridge` `snapShot.*` and `preview.*` safe host handlers.

use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener};

use serde_json::{Value, json};

use crate::bridge_types::DesktopBridgeRuntimeState;
use crate::ipc_handlers::ipc_error;
use crate::ipc_types::DesktopIpcErrorPayload;

/// Returns `true` when this host can bind `host:port`, treating missing IPv6 stack support
/// (`AddrNotAvailable` / `EAFNOSUPPORT`) as unoccupied (`#40423`, `Net.canListenOnHost`).
#[must_use]
pub fn probe_can_listen_on_host(port: u16, host: IpAddr) -> bool {
    match TcpListener::bind(SocketAddr::new(host, port)) {
        Ok(listener) => {
            drop(listener);
            true
        }
        Err(err) => {
            if err.kind() == ErrorKind::AddrNotAvailable {
                return true;
            }
            // POSIX EAFNOSUPPORT (Linux 97, macOS 47) / WinSock WSAEAFNOSUPPORT (10047).
            matches!(err.raw_os_error(), Some(47 | 97 | 10047))
        }
    }
}

/// Checks whether a local process already occupies `port` on `::1` or `::` so `localhost` would
/// connect to it instead of a `127.0.0.1` loopback tunnel forward (`#40423`).
#[must_use]
pub fn served_over_ipv6(port: u16) -> bool {
    !probe_can_listen_on_host(port, IpAddr::V6(Ipv6Addr::LOCALHOST))
        || !probe_can_listen_on_host(port, IpAddr::V6(Ipv6Addr::UNSPECIFIED))
}

/// Picks the local `127.0.0.1` port to advertise for forwarding `remote_port`, falling back to a
/// free ephemeral port whenever `remote_port` is occupied on `127.0.0.1`, `::1`, or `::` (`#40423`).
pub fn resolve_local_forward_port(remote_port: u16) -> Result<u16, String> {
    let preferred = if remote_port == 0 || served_over_ipv6(remote_port) {
        0
    } else {
        remote_port
    };
    let listener = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), preferred))
        .or_else(|_| TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)))
        .map_err(|err| format!("failed probing local loopback forward port: {err}"))?;
    let local_port = listener
        .local_addr()
        .map_err(|err| format!("failed reading probed forward port: {err}"))?
        .port();
    Ok(local_port)
}

pub(crate) fn dispatch_snapshot(
    method: &str,
    params: &Value,
    state: &mut DesktopBridgeRuntimeState,
) -> Option<Result<Value, DesktopIpcErrorPayload>> {
    match method {
        "desktop.bridge.getSnapShotState" => Some(Ok(snapshot_unavailable_state())),
        "desktop.bridge.requestSnapShotPermissions"
        | "desktop.bridge.setupSnapShot"
        | "desktop.bridge.setSnapShotAnimationDestination"
        | "desktop.bridge.dismissSnapShotAnimation"
        | "desktop.bridge.acknowledgeSnapShot"
        | "desktop.bridge.previewSnapShotConfig" => Some(Ok(Value::Null)),
        "desktop.bridge.setSnapShotShortcutSuppressed" => {
            state.readiness.shortcut_suppressed = params
                .as_bool()
                .or_else(|| params.get("suppressed").and_then(Value::as_bool))
                .unwrap_or(false);
            Some(Ok(Value::Null))
        }
        "desktop.bridge.applySnapShotConfig" => {
            Some(Ok(json!({ "backupPath": null, "warning": null })))
        }
        "desktop.bridge.checkSnapShotShortcut" => Some(Ok(json!({
            "available": false,
            "message": "Global snapshot shortcuts are unavailable in this desktop host."
        }))),
        "desktop.bridge.listPendingSnapShots" => Some(Ok(json!([]))),
        "desktop.bridge.readSnapShot" => Some(Err(ipc_error(
            "SnapShotUnavailable",
            "screen snapshots are unavailable in this desktop host",
        ))),
        _ => None,
    }
}

pub(crate) fn dispatch_preview(
    method: &str,
    params: &Value,
    state: &mut DesktopBridgeRuntimeState,
) -> Option<Result<Value, DesktopIpcErrorPayload>> {
    let sub = method.strip_prefix("desktop.bridge.preview.")?;
    match sub {
        "setForwardedShortcuts"
        | "closeTab"
        | "registerWebview"
        | "navigate"
        | "goBack"
        | "goForward"
        | "refresh"
        | "stopLoading"
        | "zoomIn"
        | "zoomOut"
        | "resetZoom"
        | "setZoomFactor"
        | "hardReload"
        | "setColorScheme"
        | "setAudioMuted"
        | "findInPage"
        | "stopFindInPage"
        | "setShown"
        | "setCaptureDrawn"
        | "openDevTools"
        | "clearCookies"
        | "clearCache"
        | "resolveTunnelAccess"
        | "stopPortForward"
        | "setAnnotationTheme"
        | "setAnnotationSendEnabled"
        | "annotationVoice.update"
        | "cancelPickElement"
        | "revealArtifact"
        | "copyArtifactToClipboard"
        | "pictureInPicture.open"
        | "pictureInPicture.close"
        | "recording.stopScreencast"
        | "pickElement" => Some(Ok(Value::Null)),
        "createTab" => {
            if let Some(tab_id) = params.get("tabId").and_then(Value::as_str) {
                state.preview_tabs.insert(tab_id.to_owned(), params.clone());
            }
            Some(Ok(Value::Null))
        }
        "getPreviewConfig" => Some(Ok(json!({
            "partition": "persist:default",
            "webPreferences": "contextIsolation=false",
            "preloadUrl": null,
        }))),
        "forwardPort" => {
            let env_id = params
                .get("environmentId")
                .and_then(Value::as_str)
                .unwrap_or("primary");
            let remote_port = params
                .get("remotePort")
                .and_then(Value::as_u64)
                .and_then(|p| u16::try_from(p).ok())
                .unwrap_or(0);
            let local_port = resolve_local_forward_port(remote_port).unwrap_or(remote_port);
            Some(Ok(json!({
                "environmentId": env_id,
                "remotePort": remote_port,
                "localPort": local_port,
            })))
        }
        "listPortForwards" | "listBrowserImportSources" => Some(Ok(json!([]))),
        "sharesLoopback" => {
            let env_id = params
                .as_str()
                .or_else(|| params.get("environmentId").and_then(Value::as_str))
                .unwrap_or_default();
            Some(Ok(Value::Bool(env_id == "primary")))
        }
        "importBrowserCookies" => Some(Ok(json!({
            "imported": 0,
            "skipped": 0,
            "skippedDomains": [],
        }))),
        "captureScreenshot" => Some(Ok(json!({
            "_tag": "PreviewCaptureFailure",
            "reason": "capture-unavailable",
            "stage": "screenshot",
        }))),
        "recording.startScreencast" => Some(Ok(json!({
            "_tag": "PreviewCaptureFailure",
            "reason": "capture-unavailable",
            "stage": "recording-start",
        }))),
        "recording.save" => Some(Err(ipc_error(
            "RecordingUnavailable",
            "preview recording save is not supported in this desktop host",
        ))),
        _ => None,
    }
}

fn snapshot_unavailable_state() -> Value {
    json!({
        "mode": "unavailable",
        "shortcut": { "kind": "both-shift-keys" },
        "shortcutRegistered": false,
        "shortcutCanRetry": false,
        "shortcutMessage": null,
        "message": "Screen snapshots are unavailable in this desktop host.",
    })
}
