//! Bundle emission determinism, `BUG-31` AUR checks, UTF-16LE BOM checks, and real
//! `legioncode-desktop` CLI integration tests.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use legion_desktop_shell::bundle::{
    DesktopBundleConfig, DesktopBundleManifest, emit_packaging_bundle,
    emit_packaging_bundle_with_config,
};
use legion_edge::boot_service_units::{decode_service_unit_bytes, extract_service_unit_base_dir};
use legion_edge::desktop_updates::DesktopUpdateChannel;
use serde_json::Value;

static BUNDLE_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

struct BundleTempDir {
    path: PathBuf,
}

impl BundleTempDir {
    fn new(prefix: &str) -> Result<Self, String> {
        let idx = BUNDLE_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("{prefix}-{}-{idx}", std::process::id()));
        fs::create_dir_all(&path).map_err(|e| e.to_string())?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for BundleTempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[test]
fn emit_packaging_bundle_is_deterministic_and_verifies_tauri_aur_and_services() -> Result<(), String>
{
    let out_a = BundleTempDir::new("legion-bundle-a")?;
    let out_b = BundleTempDir::new("legion-bundle-b")?;

    let manifest_a = emit_packaging_bundle(out_a.path()).map_err(|e| e.to_string())?;
    let manifest_b = emit_packaging_bundle(out_b.path()).map_err(|e| e.to_string())?;
    assert_eq!(manifest_a, manifest_b);
    assert_eq!(manifest_a.identifier, "dev.legioncode");
    assert_eq!(manifest_a.desktop_url, "legioncode://app/");
    assert_eq!(manifest_a.external_bin, vec!["binaries/legioncode"]);
    assert_eq!(manifest_a.files.len(), 8);

    for entry in &manifest_a.files {
        let path_a = out_a.path().join(&entry.relative_path);
        let path_b = out_b.path().join(&entry.relative_path);
        let bytes_a = fs::read(&path_a).map_err(|e| e.to_string())?;
        let bytes_b = fs::read(&path_b).map_err(|e| e.to_string())?;
        assert_eq!(
            bytes_a, bytes_b,
            "non-deterministic file {}",
            entry.relative_path
        );
        assert_eq!(bytes_a.len(), entry.byte_len);
    }

    let tauri_json: Value = serde_json::from_str(
        &fs::read_to_string(out_a.path().join("tauri.conf.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    assert_eq!(tauri_json["identifier"], "dev.legioncode");
    assert_eq!(tauri_json["app"]["windows"][0]["url"], "legioncode://app/");
    assert_eq!(
        tauri_json["bundle"]["externalBin"],
        serde_json::json!(["binaries/legioncode"])
    );
    assert!(
        tauri_json["app"]["security"]["csp"]
            .as_str()
            .is_some_and(|csp| csp.contains("img-src 'self' legioncode:"))
    );

    let caps: Value = serde_json::from_str(
        &fs::read_to_string(out_a.path().join("capabilities/default.json"))
            .map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    assert_eq!(caps["identifier"], "default");
    assert_eq!(
        caps["remote"]["urls"],
        serde_json::json!(["legioncode://app/*"])
    );

    let desktop = fs::read_to_string(out_a.path().join("linux/legioncode.desktop"))
        .map_err(|e| e.to_string())?;
    let pkgbuild =
        fs::read_to_string(out_a.path().join("linux/PKGBUILD")).map_err(|e| e.to_string())?;
    let launcher = fs::read_to_string(out_a.path().join("linux/legioncode-launcher.sh"))
        .map_err(|e| e.to_string())?;
    assert!(desktop.contains("StartupWMClass=dev.legioncode.LegionCode\n"));
    assert!(pkgbuild.contains("LEGIONCODE_DESKTOP_LAUNCHER_PATH=/usr/bin/legioncode"));
    assert!(launcher.contains("LEGIONCODE_DESKTOP_LAUNCHER_PATH=/usr/bin/legioncode"));

    let systemd = fs::read_to_string(out_a.path().join("services/legioncode.service"))
        .map_err(|e| e.to_string())?;
    let launchd = fs::read_to_string(out_a.path().join("services/dev.legioncode.service.plist"))
        .map_err(|e| e.to_string())?;
    let task_bytes =
        fs::read(out_a.path().join("services/legioncode-task.xml")).map_err(|e| e.to_string())?;
    assert_eq!(&task_bytes[..2], &[0xff, 0xfe]);
    let task_xml = decode_service_unit_bytes(&task_bytes);
    assert_eq!(
        extract_service_unit_base_dir(&systemd).as_deref(),
        Some("/home/legioncode/.legioncode")
    );
    assert_eq!(
        extract_service_unit_base_dir(&launchd).as_deref(),
        Some("/home/legioncode/.legioncode")
    );
    assert_eq!(
        extract_service_unit_base_dir(&task_xml).as_deref(),
        Some("/home/legioncode/.legioncode")
    );

    // Nightly channel bundle preserves BUG-31 distinct desktop ID & launcher env.
    let nightly_out = BundleTempDir::new("legion-bundle-nightly")?;
    let nightly_cfg = DesktopBundleConfig {
        channel: DesktopUpdateChannel::Nightly,
        ..DesktopBundleConfig::default()
    };
    let nightly_manifest = emit_packaging_bundle_with_config(nightly_out.path(), &nightly_cfg)
        .map_err(|e| e.to_string())?;
    assert_eq!(nightly_manifest.aur_package_name, "legioncode-nightly-bin");
    assert_eq!(
        nightly_manifest.aur_desktop_entry_id,
        "dev.legioncode.LegionCodeNightly.desktop"
    );
    let nightly_pkg =
        fs::read_to_string(nightly_out.path().join("linux/PKGBUILD")).map_err(|e| e.to_string())?;
    assert!(nightly_pkg.contains("LEGIONCODE_DESKTOP_LAUNCHER_PATH=/usr/bin/legioncode-nightly"));
    assert!(nightly_pkg.contains("dev.legioncode.LegionCodeNightly.desktop"));
    Ok(())
}

#[test]
fn legioncode_desktop_cli_supports_version_tauri_config_emit_bundle_ipc_eval_and_self_check()
-> Result<(), String> {
    let bin = env!("CARGO_BIN_EXE_legioncode-desktop");

    let ver = Command::new(bin)
        .arg("--version")
        .output()
        .map_err(|e| e.to_string())?;
    assert!(ver.status.success());
    assert!(String::from_utf8_lossy(&ver.stdout).contains(env!("CARGO_PKG_VERSION")));

    let tauri_cfg = Command::new(bin)
        .arg("--print-tauri-config")
        .output()
        .map_err(|e| e.to_string())?;
    assert!(tauri_cfg.status.success());
    let parsed_cfg: Value = serde_json::from_slice(&tauri_cfg.stdout).map_err(|e| e.to_string())?;
    assert_eq!(parsed_cfg["identifier"], "dev.legioncode");
    assert_eq!(
        parsed_cfg["bundle"]["externalBin"],
        serde_json::json!(["binaries/legioncode"])
    );

    let bundle_dir = BundleTempDir::new("legion-cli-emit")?;
    let emit_out = Command::new(bin)
        .arg("--emit-bundle")
        .arg(bundle_dir.path())
        .output()
        .map_err(|e| e.to_string())?;
    assert!(
        emit_out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&emit_out.stderr)
    );
    let emitted_manifest: DesktopBundleManifest =
        serde_json::from_slice(&emit_out.stdout).map_err(|e| e.to_string())?;
    assert_eq!(emitted_manifest.files.len(), 8);
    assert!(bundle_dir.path().join("tauri.conf.json").is_file());
    assert!(
        bundle_dir
            .path()
            .join("services/legioncode-task.xml")
            .is_file()
    );

    let ipc_out = Command::new(bin)
        .arg("--ipc-eval")
        .arg(r#"{"id":7,"method":"desktop.resolveScheme","params":{"url":"legioncode://app/%2e%2e%2fsecret"}}"#)
        .output()
        .map_err(|e| e.to_string())?;
    assert!(ipc_out.status.success());
    let ipc_val: Value = serde_json::from_slice(&ipc_out.stdout).map_err(|e| e.to_string())?;
    assert_eq!(ipc_val["id"], 7);
    assert_eq!(ipc_val["ok"], true);
    assert_eq!(ipc_val["result"]["status"], 404);
    assert_eq!(
        ipc_val["result"]["decision"]["rejected"]["reason"],
        "path_traversal"
    );

    let self_check = Command::new(bin)
        .arg("--self-check")
        .output()
        .map_err(|e| e.to_string())?;
    assert!(
        self_check.status.success(),
        "self-check failed: {}",
        String::from_utf8_lossy(&self_check.stderr)
    );
    let summary: Value = serde_json::from_slice(&self_check.stdout).map_err(|e| e.to_string())?;
    assert_eq!(summary["passed"], true);
    assert_eq!(summary["schemeTraversalRejected"], true);
    assert_eq!(summary["sidecarEnvSanitized"], true);
    assert_eq!(summary["retainedV1AfterV2Failure"], "1.1.0");
    assert_eq!(summary["preflightChecks"]["cleanDbReady"], true);
    assert_eq!(
        summary["preflightChecks"]["unsupportedProtocolBlocked"],
        true
    );
    assert_eq!(summary["preflightChecks"]["corruptDbBlocked"], true);
    assert_eq!(summary["serviceManifests"]["systemdRoundtrip"], true);
    assert_eq!(summary["serviceManifests"]["launchdRoundtrip"], true);
    assert_eq!(summary["serviceManifests"]["windowsTaskUtf16leBom"], true);
    assert_eq!(
        summary["serviceManifests"]["aurNightlyDesktopId"],
        "dev.legioncode.LegionCodeNightly.desktop"
    );
    assert_eq!(summary["controlServer"]["addressDerivationOk"], true);
    assert_eq!(summary["controlServer"]["loopbackRoundtrip"], true);
    assert_eq!(summary["controlServer"]["nativeTransportRoundtrip"], true);

    let bad_json = Command::new(bin)
        .arg("--ipc-eval")
        .arg("{not-json")
        .output()
        .map_err(|e| e.to_string())?;
    assert!(!bad_json.status.success());
    Ok(())
}
