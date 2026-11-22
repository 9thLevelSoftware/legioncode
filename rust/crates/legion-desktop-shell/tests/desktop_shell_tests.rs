//! Integration tests for `legion-desktop-shell` (`SidecarController`, `DesktopIpcRouter`,
//! `DesktopBundleManifest` / `emit_packaging_bundle`, and the `legioncode-desktop` CLI).

mod desktop_shell_tests {
    mod bridge_tests;
    mod bundle_cli_tests;
    mod ipc_tests;
    mod preload_bridge_tests;
    mod sidecar_tests;
    mod wave5_desktop_tests;
    mod window_control_tests;
}
