//! Desktop custom-scheme asset router, CSP builder, and clamped `WindowBounds` tests.

use t3_surfaces::desktop::{
    DEFAULT_MAIN_WINDOW_HEIGHT, DEFAULT_MAIN_WINDOW_WIDTH, DesktopRejectionReason,
    DesktopRouteDecision, DesktopSchemeRouter, DisplayRect, InitialWindowBounds, WindowBounds,
    build_desktop_csp, mime_type_for_path,
};

#[test]
fn desktop_scheme_router_serves_assets_spa_fallback_and_rejects_traversal() {
    let router = DesktopSchemeRouter::new("t3code", Some("clerk.t3.codes")).with_assets([
        "index.html",
        "assets/main.js",
        "style.css",
        "worker.wasm",
    ]);

    assert_eq!(
        router.route("GET", "t3code://app/assets/main.js?v=1", None),
        DesktopRouteDecision::ServeAsset {
            relative_path: "assets/main.js".to_owned(),
            mime_type: "text/javascript; charset=utf-8",
            content_security_policy: router.content_security_policy(),
            head_only: false,
        }
    );
    assert_eq!(
        router.route("HEAD", "t3code://app/worker.wasm", None),
        DesktopRouteDecision::ServeAsset {
            relative_path: "worker.wasm".to_owned(),
            mime_type: "application/wasm",
            content_security_policy: router.content_security_policy(),
            head_only: true,
        }
    );

    // SPA navigation vs missing asset vs dotted route with Accept: text/html.
    assert_eq!(
        router
            .route("GET", "t3code://app/threads/abc", Some("application/json"))
            .status(),
        200
    );
    assert!(matches!(
        router.route("GET", "t3code://app/missing.js", Some("*/*")),
        DesktopRouteDecision::Rejected {
            status: 404,
            reason: DesktopRejectionReason::NotFound,
            ..
        }
    ));
    assert!(matches!(
        router.route(
            "GET",
            "t3code://app/org.example",
            Some("text/html,application/xhtml+xml")
        ),
        DesktopRouteDecision::FallbackSpaIndex {
            relative_path: "index.html",
            ..
        }
    ));

    // Security gates: traversal, foreign origin, bad percent encoding, NUL byte, non-GET/HEAD.
    assert!(matches!(
        router.route("GET", "t3code://app/%2e%2e%2fetc/passwd", None),
        DesktopRouteDecision::Rejected {
            status: 404,
            reason: DesktopRejectionReason::PathTraversal,
            ..
        }
    ));
    assert!(matches!(
        router.route("GET", "t3code://other/assets/main.js", None),
        DesktopRouteDecision::Rejected {
            status: 404,
            reason: DesktopRejectionReason::ForeignOrigin,
            ..
        }
    ));
    assert!(matches!(
        router.route("GET", "t3code://app/%ZZ", None),
        DesktopRouteDecision::Rejected {
            status: 400,
            reason: DesktopRejectionReason::BadEncoding,
            ..
        }
    ));
    assert!(matches!(
        router.route("GET", "t3code://app/index.html%00.png", None),
        DesktopRouteDecision::Rejected {
            status: 400,
            reason: DesktopRejectionReason::BadEncoding,
            ..
        }
    ));
    assert!(matches!(
        router.route("POST", "t3code://app/index.html", None),
        DesktopRouteDecision::Rejected {
            status: 405,
            reason: DesktopRejectionReason::MethodNotAllowed,
            ..
        }
    ));
}

#[test]
fn desktop_csp_and_mime_table_match_electron_protocol() {
    let with_clerk = build_desktop_csp("t3code", Some("clerk.t3.codes"));
    assert!(with_clerk.contains("https://clerk.t3.codes"));
    assert!(with_clerk.contains("img-src 'self' t3code: blob: data: http: https:"));
    assert!(with_clerk.contains("frame-src 'self' blob: http: https:"));

    let injected = build_desktop_csp("t3code", Some("evil.test; script-src 'unsafe-eval'"));
    assert!(!injected.contains("evil.test"));
    let url_injected = build_desktop_csp("t3code", Some("https://evil.test/v1"));
    assert!(!url_injected.contains("evil.test"));

    assert_eq!(mime_type_for_path("A.CSS"), "text/css; charset=utf-8");
    assert_eq!(mime_type_for_path("icon.svg"), "image/svg+xml");
    assert_eq!(
        mime_type_for_path("bin.unknown"),
        "application/octet-stream"
    );
}

#[test]
fn window_bounds_respect_edge_tolerance_minimum_size_and_display_clamping() {
    let primary = DisplayRect {
        x: 0,
        y: 0,
        width: 1920,
        height: 1080,
    };
    let secondary = DisplayRect {
        x: 1920,
        y: 0,
        width: 1440,
        height: 900,
    };
    let displays = [primary, secondary];

    // Exact 1px negative origin & right/bottom edge tolerance is accepted on restore.
    let edge_ok = WindowBounds {
        x: -1,
        y: -1,
        width: 1922,
        height: 1082,
    };
    assert!(edge_ok.fits_within_display(primary));
    assert_eq!(
        WindowBounds::resolve_initial(Some(edge_ok), &displays),
        InitialWindowBounds::Restored(edge_ok)
    );

    // 2px past edge or below minimum 400x360 falls back to 1100x780 default.
    let too_far = WindowBounds {
        x: -2,
        y: 0,
        width: 1000,
        height: 700,
    };
    assert!(!too_far.fits_within_display(primary));
    assert_eq!(
        WindowBounds::resolve_initial(Some(too_far), &displays),
        InitialWindowBounds::DefaultSize {
            width: DEFAULT_MAIN_WINDOW_WIDTH,
            height: DEFAULT_MAIN_WINDOW_HEIGHT,
        }
    );
    let too_small = WindowBounds {
        x: 10,
        y: 10,
        width: 399,
        height: 360,
    };
    assert_eq!(
        WindowBounds::resolve_initial(Some(too_small), &displays),
        InitialWindowBounds::DefaultSize {
            width: DEFAULT_MAIN_WINDOW_WIDTH,
            height: DEFAULT_MAIN_WINDOW_HEIGHT,
        }
    );

    // Oversized / off-screen bounds clamp onto the primary display cleanly.
    let clamped = WindowBounds {
        x: 9000,
        y: -500,
        width: 3000,
        height: 200,
    }
    .sanitize_clamped(&displays);
    assert_eq!(
        clamped,
        WindowBounds {
            x: 0,
            y: 0,
            width: 1920,
            height: 360
        }
    );

    // normal_on_window_display shifts normal bounds onto the current window display cleanly.
    let normal = WindowBounds {
        x: 100,
        y: 100,
        width: 900,
        height: 600,
    };
    let maximized_on_secondary = WindowBounds {
        x: 1920,
        y: 0,
        width: 1440,
        height: 900,
    };
    assert_eq!(
        normal.normal_on_window_display(maximized_on_secondary, &displays),
        WindowBounds {
            x: 2020,
            y: 100,
            width: 900,
            height: 600
        }
    );
}

#[test]
fn desktop_shortcuts_format_accelerators_detect_conflicts_and_build_zoom_menu() {
    use t3_surfaces::desktop_shortcuts::{
        DesktopKeyChord, DesktopModifier, DesktopPlatform, DesktopShortcut, DesktopZoomAction,
        same_shortcut, shortcut_registration_failure_message, shortcut_system_conflict,
        to_electron_accelerator, zoom_accelerators_for_platform,
    };

    let plus = DesktopKeyChord::new("+").with_mod().with_shift();
    assert_eq!(
        to_electron_accelerator(&plus),
        "CommandOrControl+Shift+Plus"
    );
    let up = DesktopKeyChord::new("arrowup").with_alt();
    assert_eq!(to_electron_accelerator(&up), "Alt+Up");

    let mod_c = DesktopKeyChord::new("c").with_mod();
    assert_eq!(
        shortcut_system_conflict(&mod_c).as_deref(),
        Some("This shortcut is Copy in most apps.")
    );
    let shift_only = DesktopKeyChord::new("k").with_shift();
    assert!(
        shortcut_system_conflict(&shift_only)
            .is_some_and(|msg| msg.contains("Add another modifier"))
    );
    let dual = DesktopKeyChord::new("k").with_mod().with_shift();
    assert_eq!(shortcut_system_conflict(&dual), None);

    let pair = DesktopShortcut::BothShiftKeys;
    assert_eq!(
        shortcut_registration_failure_message(&pair, true),
        "Shift + Shift is not available on this system."
    );
    let pair_alt = DesktopShortcut::ModifierPair {
        modifier: DesktopModifier::Alt,
    };
    assert_eq!(
        shortcut_registration_failure_message(&pair_alt, true),
        "Option + Option is not available on this system."
    );
    assert_eq!(
        shortcut_registration_failure_message(&pair_alt, false),
        "Alt + Alt is not available on this system."
    );
    assert!(same_shortcut(
        &pair,
        &DesktopShortcut::ModifierPair {
            modifier: DesktopModifier::Shift
        }
    ));

    let mac_zoom = zoom_accelerators_for_platform(DesktopPlatform::MacOs);
    let win_zoom = zoom_accelerators_for_platform(DesktopPlatform::Windows);
    assert!(
        mac_zoom
            .iter()
            .any(|item| item.action == DesktopZoomAction::In
                && item.accelerator == "CmdOrCtrl+Shift+="
                && !item.visible)
    );
    assert!(
        !win_zoom
            .iter()
            .any(|item| item.accelerator == "CmdOrCtrl+Shift+=")
    );
}
