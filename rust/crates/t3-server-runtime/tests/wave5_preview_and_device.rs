//! Wave 5 operational tests: Chrome 158.0.8127.36 catalog, preview resize timeout,
//! and device-hub camera probe restart (`#39669`, `#40081`, `#39764`).

use std::collections::BTreeMap;
use std::path::Path;

use t3_server_runtime::{
    ChromePlatform, DeviceHubBackend, LocalDeviceHubManager, MANAGED_CHROME_VERSION, PreviewAction,
    PreviewBrowserRuntime, ServerRuntimeError, all_chrome_archive_specs, chrome_archive_spec,
};
use tempfile::TempDir;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn managed_chrome_catalog_pins_158_0_8127_36_and_verifies_archives() {
    assert_eq!(MANAGED_CHROME_VERSION, "158.0.8127.36");
    let specs = all_chrome_archive_specs();
    assert_eq!(specs.len(), 6);
    assert_eq!(
        chrome_archive_spec(ChromePlatform::Win64).sha256,
        "9655950dfeddf565561cd64cfe8ee716ccd6482fa3798503b632219efc65a71e"
    );
    assert_eq!(
        chrome_archive_spec(ChromePlatform::Linux64).bytes,
        132_087_165
    );
    let err = chrome_archive_spec(ChromePlatform::Linux64)
        .verify_archive_bytes(b"corrupt")
        .err();
    assert!(err.is_some_and(|e| e.contains("size mismatch")));
}

#[tokio::test]
async fn preview_resize_fails_when_frame_never_draws_and_keeps_undrawn_flag() -> TestResult {
    let browser = PreviewBrowserRuntime::new();
    browser.set_resize_timeout_ms(15).await;
    browser.set_drawn_viewport(560, 760).await;

    let kept = browser
        .execute_action(PreviewAction::Resize {
            mode: "fill".to_owned(),
            width: None,
            height: None,
            measured_viewport: None,
            timeout_ms: None,
        })
        .await?;
    assert_eq!(kept["viewport"]["width"], 560);
    assert!(!browser.is_desktop_resize_undrawn().await);

    let undrawn = browser
        .execute_action(PreviewAction::Resize {
            mode: "freeform".to_owned(),
            width: Some(1440),
            height: Some(900),
            measured_viewport: None,
            timeout_ms: Some(15),
        })
        .await;
    assert!(undrawn.is_err());
    assert!(browser.is_desktop_resize_undrawn().await);

    let retried = browser
        .execute_action(PreviewAction::Resize {
            mode: "freeform".to_owned(),
            width: Some(1440),
            height: Some(900),
            measured_viewport: Some((1440, 900)),
            timeout_ms: Some(15),
        })
        .await?;
    assert_eq!(retried["viewport"]["width"], 1440);
    assert!(!browser.is_desktop_resize_undrawn().await);
    Ok(())
}

#[derive(Default)]
struct MockDeviceBackend {
    camera_help: String,
    camera_probes: u32,
    spawns: Vec<BTreeMap<String, String>>,
    stops: u32,
}

impl DeviceHubBackend for MockDeviceBackend {
    fn probe_emulator_camera_help(&mut self, _bin: &Path) -> Result<String, ServerRuntimeError> {
        self.camera_probes += 1;
        Ok(self.camera_help.clone())
    }

    fn spawn_hub(&mut self, env: &BTreeMap<String, String>) -> Result<(), ServerRuntimeError> {
        self.spawns.push(env.clone());
        Ok(())
    }

    fn stop_hub(&mut self) -> Result<(), ServerRuntimeError> {
        self.stops += 1;
        Ok(())
    }
}

#[test]
fn device_hub_restarts_only_when_emulator_camera_support_changes() -> TestResult {
    let sdk = TempDir::new()?;
    let emu_dir = sdk.path().join("emulator");
    std::fs::create_dir_all(&emu_dir)?;
    let emu_bin = emu_dir.join("emulator");
    std::fs::write(&emu_bin, b"v1")?;

    let mut manager = LocalDeviceHubManager::with_platform(false);
    let mut backend = MockDeviceBackend {
        camera_help: "emulated none".to_owned(),
        ..MockDeviceBackend::default()
    };
    let env = BTreeMap::new();

    let s1 = manager.ensure_ready(&mut backend, Some(sdk.path()), &env)?;
    assert_eq!(
        s1.emulator_extra_args.as_deref(),
        Some("-camera-front emulated -camera-back emulated")
    );
    assert_eq!(backend.camera_probes, 1);
    assert_eq!(backend.spawns.len(), 1);

    let _ = manager.ensure_ready(&mut backend, Some(sdk.path()), &env)?;
    assert_eq!(backend.camera_probes, 1);

    std::fs::write(&emu_bin, b"v1-patched")?;
    let _ = manager.ensure_ready(&mut backend, Some(sdk.path()), &env)?;
    assert_eq!(backend.camera_probes, 2);
    assert_eq!(backend.stops, 0);

    std::fs::write(&emu_bin, b"v2-imagefile-supported")?;
    backend.camera_help = "emulated imagefile:<filename> none".to_owned();
    let s2 = manager.ensure_ready(&mut backend, Some(sdk.path()), &env)?;
    assert_eq!(backend.camera_probes, 3);
    assert_eq!(backend.stops, 1);
    assert_eq!(backend.spawns.len(), 2);
    assert_eq!(s2.emulator_extra_args, None);
    Ok(())
}
