//! Real `tao` + `wry` `WebView2` / native window host with `legioncode://app` custom-protocol
//! security routing, `WindowBounds` display clamping, and `window.ipc.postMessage` bridge.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use legion_surfaces::desktop::{
    DisplayRect, MIN_MAIN_WINDOW_HEIGHT, MIN_MAIN_WINDOW_WIDTH, WindowBounds,
};
use tao::dpi::{LogicalPosition, LogicalSize};
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop, EventLoopBuilder, EventLoopProxy};
use tao::platform::run_return::EventLoopExtRunReturn;
use tao::window::{Window, WindowBuilder};
use wry::{PageLoadEvent, WebContext, WebView, WebViewBuilder};

use crate::ipc::DesktopIpcRouter;
use crate::native_dialogs::client_platform;
use crate::preload_bridge::{build_bridge_init_config, render_desktop_bridge_init_script};
use crate::sidecar::{SidecarConfig, SidecarController, SidecarMode};
use crate::window_host_protocol::{
    CustomProtocolMetrics, build_smoke_html, desktop_scheme_name, resolve_custom_protocol_request,
};
use crate::window_host_smoke::{
    RuntimeTracker, SmokeTempDir, finalize_report, handle_webview_ipc, resolve_desktop_session,
    spawn_control_server_thread, spawn_smoke_watchdog,
};
pub use crate::window_host_types::{
    BaseDirOrigin, DesktopExitReason, DesktopHostError, DesktopLaunchConfig, DesktopRunReport,
    DesktopSidecarPolicy, DesktopUserEvent, WebviewSmokeChecks,
};

struct WebviewRuntimeSpec<'a> {
    config: &'a DesktopLaunchConfig,
    initial_url: &'a str,
    init_script: Option<&'a str>,
    router: &'a Arc<Mutex<DesktopIpcRouter>>,
    tracker: &'a Arc<Mutex<RuntimeTracker>>,
    proxy: &'a EventLoopProxy<DesktopUserEvent>,
}

/// Launches the real `tao` + `wry` desktop window (interactive or `--smoke-webview`).
pub fn launch_desktop_window(
    config: &DesktopLaunchConfig,
) -> Result<DesktopRunReport, DesktopHostError> {
    let session = resolve_desktop_session(config)?;
    let init_script = if config.headless_smoke {
        None
    } else {
        let bridge_cfg = build_bridge_init_config(
            session.is_development,
            &config.app_version,
            session.http_base_url.clone(),
            session.ws_base_url.clone(),
            session.bootstrap_token.clone(),
        );
        Some(
            render_desktop_bridge_init_script(&bridge_cfg)
                .map_err(DesktopHostError::WebViewCreate)?,
        )
    };

    let mut event_loop = build_event_loop(config);
    let proxy = event_loop.create_proxy();
    let clamped_bounds = clamp_loop_monitors(&event_loop, config);
    let window = create_desktop_window(&event_loop, config, clamped_bounds)?;

    let fallback_web_dir: Option<PathBuf> = (!config.headless_smoke
        && config.web_data_dir.is_none())
    .then(|| config.base_dir.join("userdata").join("webview-data"));
    let data_dir_guard = SmokeTempDir::new(
        config
            .web_data_dir
            .as_deref()
            .or(fallback_web_dir.as_deref()),
        config.headless_smoke,
    )?;
    let mut web_context = WebContext::new(Some(data_dir_guard.path.clone()));
    let tracker = Arc::new(Mutex::new(RuntimeTracker::default()));
    let router = Arc::new(Mutex::new(
        DesktopIpcRouter::new(SidecarController::new(SidecarConfig {
            base_dir: config.base_dir.clone(),
            mode: SidecarMode::from_is_development(session.is_development),
            base_dir_is_explicit: config.base_dir_origin.is_explicit(),
            app_version: config.app_version.clone(),
            ..SidecarConfig::default()
        }))
        .with_local_bootstrap(
            session.http_base_url.clone(),
            session.ws_base_url.clone(),
            session.bootstrap_token.clone(),
        ),
    ));

    let control_thread = config
        .enable_control_server
        .then(|| spawn_control_server_thread(&config.base_dir, proxy.clone()));
    let webview = build_desktop_webview(
        &WebviewRuntimeSpec {
            config,
            initial_url: &session.initial_url,
            init_script: init_script.as_deref(),
            router: &router,
            tracker: &tracker,
            proxy: &proxy,
        },
        &window,
        &mut web_context,
    )?;
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
    drop(session.managed_sidecar);

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
    #[cfg(not(any(
        target_os = "windows",
        target_os = "linux",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd"
    )))]
    let _ = config;
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
    spec: &WebviewRuntimeSpec<'_>,
    window: &Window,
    web_context: &mut WebContext,
) -> Result<WebView, DesktopHostError> {
    let smoke_html = spec.config.headless_smoke.then(build_smoke_html);
    let proto_static_dir = spec.config.static_dir.clone();
    let proto_clerk_host = spec.config.clerk_frontend_api_hostname.clone();
    let proto_tracker = spec.tracker.clone();
    let ipc_router = spec.router.clone();
    let ipc_proxy = spec.proxy.clone();
    let ipc_tracker = spec.tracker.clone();
    let load_tracker = spec.tracker.clone();

    let mut web_builder = WebViewBuilder::new_with_web_context(web_context)
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
        .with_url(spec.initial_url);
    if let Some(script) = spec.init_script {
        web_builder = web_builder.with_initialization_script(script);
    }

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
                let script =
                    format!("window.__legionReceiveIpc && window.__legionReceiveIpc({literal});");
                let _ = webview.evaluate_script(&script);
            }
        }
        Event::UserEvent(DesktopUserEvent::RevealWindow) => {
            window.set_minimized(false);
            window.set_visible(true);
            window.set_focus();
        }
        Event::WindowEvent {
            event: WindowEvent::Resized(_),
            ..
        } => {
            let is_fullscreen = window.fullscreen().is_some();
            let changed = tracker.lock().is_ok_and(|mut guard| {
                if guard.last_fullscreen == is_fullscreen {
                    false
                } else {
                    guard.last_fullscreen = is_fullscreen;
                    true
                }
            });
            if changed {
                let script = format!(
                    "window.__legionEmitDesktopEvent && window.__legionEmitDesktopEvent(\"windowFullscreenState\", {is_fullscreen});"
                );
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
                    "window.__legionActivateWorkspace && window.__legionActivateWorkspace({payload});"
                );
                let _ = webview.evaluate_script(&script);
            }
            if let Ok(activation) = serde_json::to_string(&serde_json::json!({
                "version": 1,
                "requestId": request_id,
                "type": "open-workspace",
                "workspaceRoot": workspace_root,
                "platform": client_platform(),
            })) {
                let script = format!(
                    "window.__legionEmitDesktopEvent && window.__legionEmitDesktopEvent(\"appActivationRequest\", {activation});"
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
