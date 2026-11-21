//! Deterministic Tauri v2 configuration, AUR packaging, and cross-platform boot-service
//! bundle emitter (`BUG-31` and `t3code-task.xml` UTF-16LE BOM).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use t3_edge::boot_service_units::{
    BOOT_SERVICE_LOG_FILE, BOOT_SERVICE_PLIST_FILE, BOOT_SERVICE_TASK_FILE, BOOT_SERVICE_UNIT_FILE,
    BootServicePlan, BootServiceRenderError, encode_windows_task_utf16le_bom,
    render_boot_service_launchd_plist, render_boot_service_systemd_unit,
    render_boot_service_windows_task_xml,
};
use t3_edge::desktop_updates::{DesktopUpdateChannel, render_aur_launcher};
use t3_surfaces::desktop::{DESKTOP_HOST, DESKTOP_PRODUCTION_SCHEME, build_desktop_csp};
use thiserror::Error;

use crate::bundle_templates::{
    build_default_capability_document, build_tauri_config_document, ordered_bundle_files,
    render_aur_pkgbuild,
};

/// Errors produced while building or writing a desktop packaging bundle.
#[derive(Debug, Error)]
pub enum BundleError {
    /// Systemd rejected an unsafe executable or T3 home path.
    #[error(transparent)]
    BootServiceRender(#[from] BootServiceRenderError),
    /// JSON serialization of `tauri.conf.json` or `capabilities/default.json` failed.
    #[error("failed serializing desktop bundle JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// Creating directories or writing a bundle artifact failed.
    #[error("bundle I/O failed at '{}': {source}", path.display())]
    Io {
        /// Target filesystem path.
        path: PathBuf,
        /// Underlying OS I/O error.
        #[source]
        source: io::Error,
    },
}

/// Deterministic input configuration for [`DesktopBundleManifest::generate`] and
/// [`emit_packaging_bundle_with_config`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopBundleConfig {
    /// Window and bundle product name (`"T3 Code"`).
    pub product_name: String,
    /// Package semantic version (`env!("CARGO_PKG_VERSION")`).
    pub version: String,
    /// Reverse-DNS Tauri bundle identifier (`"com.t3tools.t3code"`).
    pub identifier: String,
    /// Custom renderer scheme (`"t3code"`).
    pub scheme: String,
    /// Release channel governing AUR package IDs and `.desktop` `StartupWMClass` (`BUG-31`).
    pub channel: DesktopUpdateChannel,
    /// Optional Clerk frontend API hostname appended to CSP `script-src`.
    pub clerk_frontend_api_hostname: Option<String>,
    /// Bundled web assets relative directory in `tauri.conf.json`.
    pub frontend_dist: String,
    /// Vite dev URL recorded in `tauri.conf.json`.
    pub dev_url: String,
    /// Tauri `bundle.externalBin` sidecar paths (`["binaries/t3"]`).
    pub external_bin: Vec<String>,
    /// Canonical T3 home path embedded into service unit templates.
    pub base_dir: String,
    /// Service log path embedded into service unit templates.
    pub log_path: String,
    /// Systemd user unit installation path.
    pub systemd_unit_path: String,
    /// Sidecar launcher command and arguments for OS service definitions.
    pub service_program: Vec<String>,
    /// macOS `WorkingDirectory` home directory.
    pub launchd_home_dir: String,
    /// macOS `PATH` value inside `launchd` plist.
    pub launchd_env_path: String,
    /// Windows Task Scheduler `<UserId>`.
    pub windows_user_id: String,
    /// Windows Task Scheduler `<WorkingDirectory>`.
    pub windows_home_dir: String,
    /// Windows `conhost.exe` executable path.
    pub windows_conhost_path: String,
}

impl Default for DesktopBundleConfig {
    fn default() -> Self {
        let base_dir = "/home/t3/.t3".to_owned();
        let log_path = format!("{base_dir}/userdata/logs/{BOOT_SERVICE_LOG_FILE}");
        let systemd_unit_path = format!("/home/t3/.config/systemd/user/{BOOT_SERVICE_UNIT_FILE}");
        Self {
            product_name: "T3 Code".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            identifier: "com.t3tools.t3code".to_owned(),
            scheme: DESKTOP_PRODUCTION_SCHEME.to_owned(),
            channel: DesktopUpdateChannel::Latest,
            clerk_frontend_api_hostname: None,
            frontend_dist: "../dist".to_owned(),
            dev_url: "http://localhost:5173".to_owned(),
            external_bin: vec!["binaries/t3".to_owned()],
            base_dir,
            log_path,
            systemd_unit_path,
            service_program: vec!["/usr/bin/t3".to_owned(), "__service-launcher".to_owned()],
            launchd_home_dir: "/Users/t3".to_owned(),
            launchd_env_path: "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin".to_owned(),
            windows_user_id: "DESKTOP\\t3".to_owned(),
            windows_home_dir: "C:\\Users\\t3".to_owned(),
            windows_conhost_path: "C:\\Windows\\System32\\conhost.exe".to_owned(),
        }
    }
}

/// Deterministic integrity entry describing one emitted packaging bundle file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopBundleFileEntry {
    /// Forward-slash relative path inside the bundle output directory.
    pub relative_path: String,
    /// Exact byte length written to disk.
    pub byte_len: usize,
    /// Lowercase SHA-256 hex digest of the emitted bytes.
    pub sha256: String,
    /// Text/byte encoding (`"utf-8"` or `"utf-16le-bom"`).
    pub encoding: String,
}

/// In-memory rendered payloads for a Tauri v2 + AUR + boot-service packaging bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopBundleArtifacts {
    /// Formatted `tauri.conf.json` string.
    pub tauri_conf_json: String,
    /// Formatted `capabilities/default.json` string.
    pub capabilities_default_json: String,
    /// XDG desktop file (`linux/t3code.desktop`).
    pub linux_desktop_entry: String,
    /// Arch Linux AUR `PKGBUILD` (`linux/PKGBUILD`).
    pub linux_pkgbuild: String,
    /// POSIX `/bin/sh` wrapper script setting `T3CODE_DESKTOP_LAUNCHER_PATH` (`BUG-31`).
    pub linux_launcher_script: String,
    /// Linux `systemd` user unit (`services/t3code.service`).
    pub systemd_unit: String,
    /// macOS `launchd` user agent plist (`services/com.t3tools.t3code.service.plist`).
    pub launchd_plist: String,
    /// Windows Task Scheduler XML text (before UTF-16LE BOM encoding).
    pub windows_task_xml: String,
    /// Windows Task Scheduler XML bytes encoded as `UTF-16LE` with `0xFF 0xFE` BOM.
    pub windows_task_utf16le_bom: Vec<u8>,
}

impl DesktopBundleArtifacts {
    /// Returns all bundle files in deterministic emission order: `(relative_path, encoding, bytes)`.
    pub fn ordered_files(&self) -> Vec<(&'static str, &'static str, Vec<u8>)> {
        ordered_bundle_files(self)
    }
}

/// Deterministic summary manifest for an emitted Tauri v2 desktop packaging bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopBundleManifest {
    /// Manifest format version (`1`).
    pub schema_version: u32,
    /// Product display name (`"T3 Code"`).
    pub product_name: String,
    /// Bundle semantic version.
    pub version: String,
    /// Bundle reverse-DNS identifier (`"com.t3tools.t3code"`).
    pub identifier: String,
    /// Custom protocol scheme (`"t3code"`).
    pub scheme: String,
    /// Desktop entry origin URL (`"t3code://app/"`).
    pub desktop_url: String,
    /// Generated `Content-Security-Policy` header string.
    pub content_security_policy: String,
    /// Bundled sidecar binaries (`["binaries/t3"]`).
    pub external_bin: Vec<String>,
    /// Update channel used when generating AUR manifests.
    pub channel: DesktopUpdateChannel,
    /// AUR package name (`t3code-bin` or `t3code-nightly-bin`).
    pub aur_package_name: String,
    /// XDG desktop file ID (`com.t3tools.T3Code.desktop` or `com.t3tools.T3CodeNightly.desktop`).
    pub aur_desktop_entry_id: String,
    /// Ordered deterministic file inventory with byte sizes and SHA-256 digests.
    pub files: Vec<DesktopBundleFileEntry>,
}

impl DesktopBundleManifest {
    /// Builds deterministic packaging bundle manifest and artifacts from `config`.
    pub fn generate(
        config: &DesktopBundleConfig,
    ) -> Result<(Self, DesktopBundleArtifacts), BundleError> {
        let csp = build_desktop_csp(
            &config.scheme,
            config.clerk_frontend_api_hostname.as_deref(),
        );
        let tauri_conf = build_tauri_config_document(config, &csp);
        let mut tauri_conf_json = serde_json::to_string_pretty(&tauri_conf)?;
        tauri_conf_json.push('\n');

        let cap_doc = build_default_capability_document(&config.scheme);
        let mut capabilities_default_json = serde_json::to_string_pretty(&cap_doc)?;
        capabilities_default_json.push('\n');

        let aur = render_aur_launcher(config.channel);
        let linux_pkgbuild = render_aur_pkgbuild(&config.version, config.channel, &aur);

        let plan = BootServicePlan::new(
            config.service_program.clone(),
            &config.base_dir,
            &config.log_path,
            &config.systemd_unit_path,
        );
        let systemd_unit = render_boot_service_systemd_unit(&plan)?;
        let launchd_plist = render_boot_service_launchd_plist(
            &plan,
            &config.launchd_home_dir,
            &config.launchd_env_path,
        );
        let windows_task_xml = render_boot_service_windows_task_xml(
            &plan,
            &config.windows_user_id,
            &config.windows_home_dir,
            &config.windows_conhost_path,
        );
        let windows_task_utf16le_bom = encode_windows_task_utf16le_bom(&windows_task_xml);

        let artifacts = DesktopBundleArtifacts {
            tauri_conf_json,
            capabilities_default_json,
            linux_desktop_entry: aur.desktop_entry,
            linux_pkgbuild,
            linux_launcher_script: aur.launcher_script,
            systemd_unit,
            launchd_plist,
            windows_task_xml,
            windows_task_utf16le_bom,
        };

        let files = artifacts
            .ordered_files()
            .into_iter()
            .map(|(rel, encoding, bytes)| DesktopBundleFileEntry {
                relative_path: rel.to_owned(),
                byte_len: bytes.len(),
                sha256: hex::encode(Sha256::digest(&bytes)),
                encoding: encoding.to_owned(),
            })
            .collect();

        let manifest = Self {
            schema_version: 1,
            product_name: config.product_name.clone(),
            version: config.version.clone(),
            identifier: config.identifier.clone(),
            scheme: config.scheme.clone(),
            desktop_url: format!("{}://{DESKTOP_HOST}/", config.scheme),
            content_security_policy: csp,
            external_bin: config.external_bin.clone(),
            channel: config.channel,
            aur_package_name: aur.package_name.to_owned(),
            aur_desktop_entry_id: aur.desktop_entry_id.to_owned(),
            files,
        };

        Ok((manifest, artifacts))
    }

    /// Builds the default production packaging bundle manifest and artifacts.
    pub fn default_bundle() -> Result<(Self, DesktopBundleArtifacts), BundleError> {
        Self::generate(&DesktopBundleConfig::default())
    }
}

/// Renders the deterministic `tauri.conf.json` string for `config`.
pub fn render_tauri_config_json(config: &DesktopBundleConfig) -> Result<String, BundleError> {
    let (_, artifacts) = DesktopBundleManifest::generate(config)?;
    Ok(artifacts.tauri_conf_json)
}

/// Emits the complete default packaging bundle into `out_dir` and returns its [`DesktopBundleManifest`].
pub fn emit_packaging_bundle(out_dir: &Path) -> Result<DesktopBundleManifest, BundleError> {
    emit_packaging_bundle_with_config(out_dir, &DesktopBundleConfig::default())
}

/// Emits a deterministic packaging bundle configured by `config` into `out_dir`.
pub fn emit_packaging_bundle_with_config(
    out_dir: &Path,
    config: &DesktopBundleConfig,
) -> Result<DesktopBundleManifest, BundleError> {
    let (manifest, artifacts) = DesktopBundleManifest::generate(config)?;
    for (relative_path, _encoding, bytes) in artifacts.ordered_files() {
        let mut target = out_dir.to_path_buf();
        for segment in relative_path.split('/') {
            target.push(segment);
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|source| BundleError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        fs::write(&target, bytes).map_err(|source| BundleError::Io {
            path: target,
            source,
        })?;
    }
    debug_assert_eq!(BOOT_SERVICE_UNIT_FILE, "t3code.service");
    debug_assert_eq!(BOOT_SERVICE_PLIST_FILE, "com.t3tools.t3code.service.plist");
    debug_assert_eq!(BOOT_SERVICE_TASK_FILE, "t3code-task.xml");
    Ok(manifest)
}
