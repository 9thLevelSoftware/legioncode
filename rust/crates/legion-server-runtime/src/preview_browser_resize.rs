//! Desktop preview pane resize frame-draw confirmation helper (`#40081`).

use serde_json::{Value, json};
use tokio::sync::Mutex as TokioMutex;

use crate::error::ServerRuntimeError;
use crate::preview_cdp::CdpConnection;

const DEFAULT_DESKTOP_RESIZE_TIMEOUT_MS: u64 = 1_500;

/// Tracks viewport paint state across [`crate::preview_browser::PreviewAction::Resize`] requests (`#40081`).
#[derive(Default)]
pub(crate) struct PreviewResizeState {
    pub current_viewport: Option<(u32, u32)>,
    pub desktop_resize_undrawn: bool,
    pub resize_timeout_ms: Option<u64>,
}

pub(crate) async fn execute_resize<F, Fut>(
    resize_lock: &TokioMutex<PreviewResizeState>,
    set_metrics: F,
    mode: &str,
    width: Option<u32>,
    height: Option<u32>,
    measured_viewport: Option<(u32, u32)>,
    timeout_ms: Option<u64>,
) -> Result<Value, ServerRuntimeError>
where
    F: FnOnce(u32, u32) -> Fut,
    Fut: std::future::Future<Output = Result<(), ServerRuntimeError>>,
{
    let target = if mode == "fill" {
        None
    } else {
        Some((
            width.ok_or_else(|| {
                ServerRuntimeError::BadRequest("freeform resize requires width".to_owned())
            })?,
            height.ok_or_else(|| {
                ServerRuntimeError::BadRequest("freeform resize requires height".to_owned())
            })?,
        ))
    };

    let (changed, wait_ms, prev_viewport) = {
        let state = resize_lock.lock().await;
        let changed = state.desktop_resize_undrawn
            || match (state.current_viewport, target) {
                (_, None) => false,
                (prev, Some(next)) => prev != Some(next),
            };
        let wait_ms = timeout_ms
            .or(state.resize_timeout_ms)
            .unwrap_or(DEFAULT_DESKTOP_RESIZE_TIMEOUT_MS);
        (changed, wait_ms, state.current_viewport)
    };

    if !changed {
        let vp = prev_viewport.unwrap_or((1280, 720));
        return Ok(json!({ "viewport": { "width": vp.0, "height": vp.1 } }));
    }

    let to = target
        .ok_or_else(|| ServerRuntimeError::BadRequest("missing target viewport".to_owned()))?;
    set_metrics(to.0, to.1).await?;

    if let Some(measured) = measured_viewport {
        let mut state = resize_lock.lock().await;
        if measured == to {
            state.current_viewport = Some(measured);
            state.desktop_resize_undrawn = false;
            return Ok(json!({ "viewport": { "width": measured.0, "height": measured.1 } }));
        }
        state.desktop_resize_undrawn = true;
        return Err(undrawn_resize_error());
    }

    tokio::time::sleep(std::time::Duration::from_millis(wait_ms)).await;
    let mut state = resize_lock.lock().await;
    if state.current_viewport == Some(to) {
        state.desktop_resize_undrawn = false;
        return Ok(json!({ "viewport": { "width": to.0, "height": to.1 } }));
    }
    state.desktop_resize_undrawn = true;
    Err(undrawn_resize_error())
}

pub(crate) async fn notify_cdp_metrics_optional(
    cdp: Option<&mut CdpConnection>,
    width: u32,
    height: u32,
) -> Result<(), ServerRuntimeError> {
    if let Some(conn) = cdp {
        let _ = conn.set_device_metrics(width, height, 1.0, false).await?;
    }
    Ok(())
}

fn undrawn_resize_error() -> ServerRuntimeError {
    ServerRuntimeError::Conflict(format!(
        "The desktop did not draw the page at the new size. Show or restore the {} window and try again.",
        legion_contracts::brand::ACTIVE_BRAND.product_name
    ))
}
