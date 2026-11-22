//! `window.desktopBridge` `snapShot.*` and `preview.*` safe host handlers.

use serde_json::{Value, json};

use crate::bridge_types::DesktopBridgeRuntimeState;
use crate::ipc_handlers::ipc_error;
use crate::ipc_types::DesktopIpcErrorPayload;

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
                .unwrap_or(0);
            Some(Ok(json!({
                "environmentId": env_id,
                "remotePort": remote_port,
                "localPort": remote_port,
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
