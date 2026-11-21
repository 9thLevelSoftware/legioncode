//! Real `tao` + `wry` `WebView2` / native window host with `t3code://app` custom-protocol
//! security routing, `WindowBounds` display clamping, and `window.ipc.postMessage` bridge.

use std::sync::{Arc, Mutex};

use t3_surfaces::desktop::{
    DisplayRect, MIN_MAIN_WINDOW_HEIGHT, MIN_MAIN_WINDOW_WIDTH, WindowBounds,
};
use tao::dpi::{LogicalPosition, LogicalSize};
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop, EventLoopBuilder, EventLoopProxy};
use tao::platform::run_return::EventLoopExtRunReturn;
use tao::window::{Window, WindowBuilder};
use wry::{PageLoadEvent, WebContext, WebView, WebViewBuilder};

use crate::ipc::{DesktopIpcRequest, DesktopIpcRouter};
use crate::sidecar::{SidecarConfig, SidecarController};
use crate::window_host_protocol::{
    CustomProtocolMetrics, build_smoke_html, desktop_scheme_name, resolve_custom_protocol_request,
};
use crate::window_host_smoke::{
    RuntimeTracker, SmokeTempDir, finalize_report, spawn_control_server_thread,
    spawn_smoke_watchdog,
};
pub use crate::window_host_types::{
    DesktopExitReason, DesktopHostError, DesktopLaunchConfig, DesktopRunReport, DesktopUserEvent,
    WebviewSmokeChecks,
};

/// Launches the real `tao` + `wry` desktop window (interactive or `--smoke-webview`).
pub fn launch_desktop_window(
    config: &DesktopLaunchConfig,
) -> Result<DesktopRunReport, DesktopHostError> {
    let mut event_loop = build_event_loop(config);
    let proxy = event_loop.create_proxy();
    let clamped_bounds = clamp_loop_monitors(&event_loop, config);
    let window = create_desktop_window(&event_loop, config, clamped_bounds)?;

    let data_dir_guard = SmokeTempDir::new(config.web_data_dir.as_deref(), config.headless_smoke)?;
    let mut web_context = WebContext::new(Some(data_dir_guard.path.clone()));
    let tracker = Arc::new(Mutex::new(RuntimeTracker::default()));
    let router = Arc::new(Mutex::new(DesktopIpcRouter::new(SidecarController::new(
        SidecarConfig {
            base_dir: config.base_dir.clone(),
            app_version: config.app_version.clone(),
            ..SidecarConfig::default()
        },
    ))));

    let control_thread = config
        .enable_control_server
        .then(|| spawn_control_server_thread(&config.base_dir, proxy.clone()));
    let webview =
        build_desktop_webview(config, &window, &mut web_context, &router, &tracker, &proxy)?;
    let watchdog = config
        .headless_smoke
        .then(|| spawn_smoke_watchdog(config.smoke_timeout_ms, proxy));

    let event_tracker = tracker.clone();
    event_loop.run_return(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        handle_loop_event(&event, &window, &webview, &event_tracker, control_flow);
    });

    if let Some((cancel_tx, handle)) = watchdog {
        let _ = cancel_tx.send(());
        let _ = handle.join();
    }
    if let Some((stop_tx, handle)) = control_thread {
        let _ = stop_tx.send(());
        let _ = handle.join();
    }
    drop(web_context);

    let snapshot = tracker
        .lock()
        .map_err(|_| DesktopHostError::WebViewCreate("runtime tracker lock poisoned".to_owned()))?;
    Ok(finalize_report(config, clamped_bounds, &snapshot))
}

fn build_event_loop(config: &DesktopLaunchConfig) -> EventLoop<DesktopUserEvent> {
    let mut builder = EventLoopBuilder::<DesktopUserEvent>::with_user_event();
    #[cfg(target_os = "windows")]
    {
        use tao::platform::windows::EventLoopBuilderExtWindows;
        builder.with_any_thread(config.allow_any_thread);
    }
    #[cfg(any(
        target_os = "linux",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd"
    ))]
    {
        use tao::platform::unix::EventLoopBuilderExtUnix;
        builder.with_any_thread(config.allow_any_thread);
    }
    builder.build()
}

fn clamp_loop_monitors(
    event_loop: &EventLoop<DesktopUserEvent>,
    config: &DesktopLaunchConfig,
) -> WindowBounds {
    let mut monitors: Vec<DisplayRect> = event_loop
        .available_monitors()
        .map(|mon| {
            let scale = mon.scale_factor();
            let pos = mon.position().to_logical::<i32>(scale);
            let size = mon.size().to_logical::<i32>(scale);
            DisplayRect {
                x: pos.x,
                y: pos.y,
                width: size.width,
                height: size.height,
            }
        })
        .filter(|d| d.width > 0 && d.height > 0)
        .collect();
    if monitors.is_empty() {
        monitors.clone_from(&config.displays);
    }
    config.bounds.sanitize_clamped(&monitors)
}

fn create_desktop_window(
    event_loop: &EventLoop<DesktopUserEvent>,
    config: &DesktopLaunchConfig,
    clamped_bounds: WindowBounds,
) -> Result<Window, DesktopHostError> {
    WindowBuilder::new()
        .with_title(&config.title)
        .with_inner_size(LogicalSize::new(
            f64::from(clamped_bounds.width),
            f64::from(clamped_bounds.height),
        ))
        .with_position(LogicalPosition::new(
            f64::from(clamped_bounds.x),
            f64::from(clamped_bounds.y),
        ))
        .with_min_inner_size(LogicalSize::new(
            f64::from(MIN_MAIN_WINDOW_WIDTH),
            f64::from(MIN_MAIN_WINDOW_HEIGHT),
        ))
        .with_visible(!config.headless_smoke)
        .build(event_loop)
        .map_err(|err| DesktopHostError::WindowCreate(err.to_string()))
}

fn build_desktop_webview(
    config: &DesktopLaunchConfig,
    window: &Window,
    web_context: &mut WebContext,
    router: &Arc<Mutex<DesktopIpcRouter>>,
    tracker: &Arc<Mutex<RuntimeTracker>>,
    proxy: &EventLoopProxy<DesktopUserEvent>,
) -> Result<WebView, DesktopHostError> {
    let smoke_html = config.headless_smoke.then(build_smoke_html);
    let proto_static_dir = config.static_dir.clone();
    let proto_clerk_host = config.clerk_frontend_api_hostname.clone();
    let proto_tracker = tracker.clone();
    let ipc_router = router.clone();
    let ipc_proxy = proxy.clone();
    let ipc_tracker = tracker.clone();
    let load_tracker = tracker.clone();

    let web_builder = WebViewBuilder::new_with_web_context(web_context)
        .with_custom_protocol(desktop_scheme_name().to_owned(), move |_id, request| {
            let mut local_metrics = CustomProtocolMetrics::default();
            let resp = resolve_custom_protocol_request(
                desktop_scheme_name(),
                proto_clerk_host.as_deref(),
                proto_static_dir.as_deref(),
                smoke_html,
                &request,
                Some(&mut local_metrics),
            );
            if let Ok(mut guard) = proto_tracker.lock() {
                guard.protocol_metrics.served_ok |= local_metrics.served_ok;
                guard.protocol_metrics.traversal_rejected |= local_metrics.traversal_rejected;
            }
            resp
        })
        .with_ipc_handler(move |request| {
            handle_webview_ipc(request.body(), &ipc_router, &ipc_tracker, &ipc_proxy);
        })
        .with_on_page_load_handler(move |event, _url| {
            if matches!(event, PageLoadEvent::Finished)
                && let Ok(mut guard) = load_tracker.lock()
            {
                guard.webview_loaded = true;
            }
        })
        .with_url(&config.initial_url);

    #[cfg(target_os = "linux")]
    {
        use tao::platform::unix::WindowExtUnix;
        use wry::WebViewBuilderExtUnix;
        let container = window.default_vbox().ok_or_else(|| {
            DesktopHostError::WebViewCreate("missing GTK default vbox".to_owned())
        })?;
        web_builder
            .build_gtk(container)
            .map_err(|err| DesktopHostError::WebViewCreate(err.to_string()))
    }
    #[cfg(not(target_os = "linux"))]
    {
        web_builder
            .build(window)
            .map_err(|err| DesktopHostError::WebViewCreate(err.to_string()))
    }
}

fn handle_loop_event(
    event: &Event<'_, DesktopUserEvent>,
    window: &Window,
    webview: &WebView,
    tracker: &Arc<Mutex<RuntimeTracker>>,
    control_flow: &mut ControlFlow,
) {
    match event {
        Event::WindowEvent {
            event: WindowEvent::CloseRequested,
            ..
        }
        | Event::UserEvent(DesktopUserEvent::SmokeComplete) => {
            *control_flow = ControlFlow::Exit;
        }
        Event::UserEvent(DesktopUserEvent::IpcReply(reply_json)) => {
            if let Ok(literal) = serde_json::to_string(reply_json) {
                let script = format!("window.__t3ReceiveIpc && window.__t3ReceiveIpc({literal});");
                let _ = webview.evaluate_script(&script);
            }
        }
        Event::UserEvent(DesktopUserEvent::ActivateWorkspace {
            request_id,
            workspace_root,
        }) => {
            window.set_minimized(false);
            window.set_visible(true);
            window.set_focus();
            if let Ok(mut guard) = tracker.lock() {
                guard.activated_workspaces.push(workspace_root.clone());
            }
            if let Ok(payload) = serde_json::to_string(&serde_json::json!({
                "requestId": request_id,
                "workspaceRoot": workspace_root,
            })) {
                let script = format!(
                    "window.__t3ActivateWorkspace && window.__t3ActivateWorkspace({payload});"
                );
                let _ = webview.evaluate_script(&script);
            }
        }
        Event::UserEvent(DesktopUserEvent::SmokeTimeout) => {
            if let Ok(mut guard) = tracker.lock() {
                guard.smoke_timed_out = true;
            }
            *control_flow = ControlFlow::Exit;
        }
        _ => {}
    }
}

fn handle_webview_ipc(
    body: &str,
    router: &Arc<Mutex<DesktopIpcRouter>>,
    tracker: &Arc<Mutex<RuntimeTracker>>,
    proxy: &EventLoopProxy<DesktopUserEvent>,
) {
    let Ok(req) = serde_json::from_str::<DesktopIpcRequest>(body) else {
        return;
    };
    if req.method == "desktop.__smokeComplete" {
        if let Ok(mut guard) = tracker.lock() {
            guard.smoke_signal.js_smoke_ok =
                req.params.get("ok").and_then(serde_json::Value::as_bool) == Some(true);
            guard.smoke_signal.js_traversal_rejected = req
                .params
                .get("traversalRejected")
                .and_then(serde_json::Value::as_bool)
                == Some(true);
            guard.js_fetch_traversal_status = req
                .params
                .get("fetchTraversalStatus")
                .and_then(serde_json::Value::as_u64)
                .and_then(|n| u16::try_from(n).ok())
                .unwrap_or(0);
            guard.smoke_signal.smoke_completed = true;
        }
        let _ = proxy.send_event(DesktopUserEvent::SmokeComplete);
        return;
    }

    let reply_json = {
        let Ok(mut guard) = router.lock() else {
            return;
        };
        serde_json::to_string(&guard.dispatch(req)).ok()
    };
    if let Some(json) = reply_json {
        if let Ok(mut guard) = tracker.lock() {
            guard.ipc_round_trips += 1;
        }
        let _ = proxy.send_event(DesktopUserEvent::IpcReply(json));
    }
}
