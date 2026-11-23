//! Wave 6 desktop tests: proxy `403` Origin stripping (`#41092`), Win32/native context menu
//! ampersand escaping (`#41097`), WSL install-stub registry check (`#40918`), `AppImage` CLI shim
//! systemd `SIGTERM` & fd-9 stdin handling (`#40624`), and IPv6 loopback port-forward probing
//! (`#40423`).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener};

use legion_desktop_shell::browser_webview::{
    NativeContextMenuItem, build_native_context_menu, escape_native_menu_label,
};
use legion_desktop_shell::cli_shim::render_appimage_cli_shim;
use legion_desktop_shell::ipc::{DesktopIpcRequest, DesktopIpcRouter};
use legion_desktop_shell::ipc_bridge_preview::{
    probe_can_listen_on_host, resolve_local_forward_port,
};
use legion_desktop_shell::remote_request_headers::{
    MainWindowRequestHeaderDetails, MainWindowRequestHeaderPolicy, strip_desktop_origin_headers,
};
use legion_desktop_shell::wsl_env::{WSL_SERVICE_KEYS, probe_wsl_availability_with};
use serde_json::json;

#[test]
fn strips_desktop_origin_and_sec_fetch_for_remote_proxies_preserving_clerk_and_dev() {
    let packaged = MainWindowRequestHeaderPolicy {
        desktop_origin: "legioncode://app".to_owned(),
        main_web_contents_id: 42,
        clerk_frontend_api_hostname: Some("clerk.legion.codes".to_owned()),
        dev_server_origin: None,
        is_development: false,
    };
    let stripped = strip_desktop_origin_headers(
        &packaged,
        &MainWindowRequestHeaderDetails {
            url: "wss://remote.example.com/ws?ticket=abc".to_owned(),
            web_contents_id: Some(42),
            is_subframe: false,
            request_headers: vec![
                ("origin".to_owned(), "legioncode://app".to_owned()),
                ("Sec-Fetch-Site".to_owned(), "cross-site".to_owned()),
                ("sec-fetch-user".to_owned(), "?1".to_owned()),
                ("Authorization".to_owned(), "Bearer secret".to_owned()),
                ("Upgrade".to_owned(), "websocket".to_owned()),
            ],
        },
    );
    assert_eq!(
        stripped,
        Some(vec![
            ("Authorization".to_owned(), "Bearer secret".to_owned()),
            ("Upgrade".to_owned(), "websocket".to_owned()),
        ])
    );

    // Preserves Clerk (case-insensitive & trailing dot).
    assert_eq!(
        strip_desktop_origin_headers(
            &packaged,
            &MainWindowRequestHeaderDetails {
                url: "https://CLERK.LEGION.CODES./v1/client".to_owned(),
                web_contents_id: Some(42),
                is_subframe: false,
                request_headers: vec![("Origin".to_owned(), "legioncode://app".to_owned())],
            },
        ),
        None
    );

    // Ignores sandboxed iframes (Origin: null), subframes, and other webContents.
    for (wc, subframe, origin) in [
        (Some(42), false, "null"),
        (Some(42), true, "legioncode://app"),
        (Some(99), false, "legioncode://app"),
    ] {
        assert_eq!(
            strip_desktop_origin_headers(
                &packaged,
                &MainWindowRequestHeaderDetails {
                    url: "https://remote.example.com/api/test".to_owned(),
                    web_contents_id: wc,
                    is_subframe: subframe,
                    request_headers: vec![("Origin".to_owned(), origin.to_owned())],
                },
            ),
            None
        );
    }
}

#[test]
fn dev_origin_header_stripping_preserves_dev_server_and_loopback() {
    let dev = MainWindowRequestHeaderPolicy {
        desktop_origin: "legioncode-dev://app".to_owned(),
        main_web_contents_id: 42,
        clerk_frontend_api_hostname: None,
        dev_server_origin: Some("http://devbox.internal:5733".to_owned()),
        is_development: true,
    };
    for preserved_url in [
        "http://localhost:3773/api/health",
        "http://app.localhost:3773/api/health",
        "http://[::1]:3773/api/health",
        "ws://devbox.internal:5733/ws",
    ] {
        assert_eq!(
            strip_desktop_origin_headers(
                &dev,
                &MainWindowRequestHeaderDetails {
                    url: preserved_url.to_owned(),
                    web_contents_id: Some(42),
                    is_subframe: false,
                    request_headers: vec![("Origin".to_owned(), "legioncode-dev://app".to_owned())],
                },
            ),
            None
        );
    }
    assert_eq!(
        strip_desktop_origin_headers(
            &dev,
            &MainWindowRequestHeaderDetails {
                url: "http://devbox.internal:8080/api/auth/bootstrap".to_owned(),
                web_contents_id: Some(42),
                is_subframe: false,
                request_headers: vec![
                    ("Origin".to_owned(), "legioncode-dev://app".to_owned()),
                    ("Sec-Fetch-Mode".to_owned(), "cors".to_owned()),
                    ("Content-Type".to_owned(), "application/json".to_owned()),
                ],
            },
        ),
        Some(vec![(
            "Content-Type".to_owned(),
            "application/json".to_owned()
        )])
    );
}

#[test]
fn escapes_ampersands_recursively_in_native_context_menu_labels() {
    assert_eq!(escape_native_menu_label("Filter by R&D"), "Filter by R&&D");
    assert_eq!(escape_native_menu_label("C && D"), "C &&&& D");
    let built = build_native_context_menu(&[
        NativeContextMenuItem {
            id: "filter".to_owned(),
            label: "Filter by R&D".to_owned(),
            disabled: false,
            checked: None,
            children: Vec::new(),
        },
        NativeContextMenuItem {
            id: "more".to_owned(),
            label: "A&B".to_owned(),
            disabled: false,
            checked: Some(true),
            children: vec![NativeContextMenuItem {
                id: "child".to_owned(),
                label: "C && D".to_owned(),
                disabled: false,
                checked: None,
                children: Vec::new(),
            }],
        },
    ]);
    assert_eq!(built[0].label, "Filter by R&&D");
    assert_eq!(built[1].label, "A&&B");
    assert_eq!(built[1].children[0].label, "C &&&& D");

    let mut router = DesktopIpcRouter::default();
    let res = router.dispatch(DesktopIpcRequest {
        id: Some(json!("cm1")),
        method: "desktop.bridge.showContextMenu".to_owned(),
        params: json!({
            "items": [{ "id": "x", "label": "Filter by R&D" }]
        }),
    });
    assert!(res.ok);
}

#[test]
fn wsl_availability_rejects_stub_without_registered_service_key() {
    let stub_only =
        probe_wsl_availability_with("win32", r"C:\Windows", |_| true, |_| false, || false);
    assert!(!stub_only);

    let with_lxss = probe_wsl_availability_with(
        "win32",
        r"C:\Windows",
        |_| true,
        |key| key == WSL_SERVICE_KEYS[1],
        || false,
    );
    assert!(with_lxss);

    let blocked_by_policy = probe_wsl_availability_with(
        "win32",
        r"C:\Windows",
        |_| true,
        |key| key == WSL_SERVICE_KEYS[0],
        || true,
    );
    assert!(!blocked_by_policy);
    assert!(!probe_wsl_availability_with(
        "linux",
        r"C:\Windows",
        |_| true,
        |_| true,
        || false,
    ));
}

#[test]
fn appimage_cli_shim_handles_systemd_sigterm_lazy_unmount_and_fd9_stdin() {
    let shim = render_appimage_cli_shim("/opt/LegionCode.AppImage", Some("/home/user/.legioncode"));
    assert!(shim.contains("trap '' TERM"));
    assert!(shim.contains("fusermount3 -uz \"$mount\""));
    assert!(shim.contains("{ command exec 9<&0; } 2>/dev/null || exec 9</dev/null"));
    assert!(shim.contains("<&9 9<&- &"));
    assert!(shim.contains("trap 'kill -TERM \"$cli\" 2>/dev/null' TERM"));
    assert!(shim.contains("while kill -0 \"$cli\" 2>/dev/null; do"));
    assert!(shim.contains("exit \"$status\""));
}

#[test]
fn forward_port_moves_off_ipv4_and_ipv6_loopback_listeners() -> Result<(), String> {
    let v4_decoy = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .map_err(|e| e.to_string())?;
    let v4_port = v4_decoy.local_addr().map_err(|e| e.to_string())?.port();
    let mut router = DesktopIpcRouter::default();
    let v4_res = router.dispatch(DesktopIpcRequest {
        id: Some(json!("f1")),
        method: "desktop.bridge.preview.forwardPort".to_owned(),
        params: json!({ "environmentId": "remote", "remotePort": v4_port }),
    });
    assert!(v4_res.ok);
    let local_v4 = v4_res
        .result
        .as_ref()
        .and_then(|v| v["localPort"].as_u64())
        .ok_or("missing localPort")?;
    assert_ne!(local_v4, u64::from(v4_port));
    assert!(local_v4 > 0);
    drop(v4_decoy);

    if probe_can_listen_on_host(0, IpAddr::V6(Ipv6Addr::LOCALHOST))
        && let Ok(v6_decoy) = TcpListener::bind(SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 0))
    {
        let v6_port = v6_decoy.local_addr().map_err(|e| e.to_string())?.port();
        let moved = resolve_local_forward_port(v6_port)?;
        assert_ne!(moved, v6_port);
        assert!(moved > 0);
    }
    Ok(())
}
