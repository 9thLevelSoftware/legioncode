//! Deterministic Tauri v2 configuration, capability, and Arch AUR `PKGBUILD` renderers.

use legion_edge::desktop_updates::{AurLauncherBundle, DesktopUpdateChannel};
use legion_surfaces::desktop::{
    DEFAULT_MAIN_WINDOW_HEIGHT, DEFAULT_MAIN_WINDOW_WIDTH, DESKTOP_HOST, MIN_MAIN_WINDOW_HEIGHT,
    MIN_MAIN_WINDOW_WIDTH,
};
use serde::{Deserialize, Serialize};

use crate::bundle::DesktopBundleConfig;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TauriBuildConfig {
    pub frontend_dist: String,
    pub dev_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TauriWindowConfig {
    pub label: String,
    pub title: String,
    pub url: String,
    pub width: i32,
    pub height: i32,
    pub min_width: i32,
    pub min_height: i32,
    pub resizable: bool,
    pub visible: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TauriSecurityConfig {
    pub csp: String,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TauriAppConfig {
    pub with_global_tauri: bool,
    pub windows: Vec<TauriWindowConfig>,
    pub security: TauriSecurityConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TauriBundleSection {
    pub active: bool,
    pub targets: String,
    pub external_bin: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeepLinkDesktopConfig {
    pub schemes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeepLinkPluginConfig {
    pub desktop: DeepLinkDesktopConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct TauriPluginsConfig {
    pub deep_link: DeepLinkPluginConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TauriConfigDocument {
    #[serde(rename = "$schema")]
    pub schema: String,
    pub product_name: String,
    pub version: String,
    pub identifier: String,
    pub build: TauriBuildConfig,
    pub app: TauriAppConfig,
    pub bundle: TauriBundleSection,
    pub plugins: TauriPluginsConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TauriRemoteUrls {
    pub urls: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TauriCapabilityDocument {
    #[serde(rename = "$schema")]
    pub schema: String,
    pub identifier: String,
    pub description: String,
    pub windows: Vec<String>,
    pub remote: TauriRemoteUrls,
    pub permissions: Vec<String>,
}

pub(crate) fn build_tauri_config_document(
    config: &DesktopBundleConfig,
    csp: &str,
) -> TauriConfigDocument {
    TauriConfigDocument {
        schema: "https://schema.tauri.app/config/2".to_owned(),
        product_name: config.product_name.clone(),
        version: config.version.clone(),
        identifier: config.identifier.clone(),
        build: TauriBuildConfig {
            frontend_dist: config.frontend_dist.clone(),
            dev_url: config.dev_url.clone(),
        },
        app: TauriAppConfig {
            with_global_tauri: false,
            windows: vec![TauriWindowConfig {
                label: "main".to_owned(),
                title: config.product_name.clone(),
                url: format!("{}://{DESKTOP_HOST}/", config.scheme),
                width: DEFAULT_MAIN_WINDOW_WIDTH,
                height: DEFAULT_MAIN_WINDOW_HEIGHT,
                min_width: MIN_MAIN_WINDOW_WIDTH,
                min_height: MIN_MAIN_WINDOW_HEIGHT,
                resizable: true,
                visible: false,
            }],
            security: TauriSecurityConfig {
                csp: csp.to_owned(),
                capabilities: vec!["default".to_owned()],
            },
        },
        bundle: TauriBundleSection {
            active: true,
            targets: "all".to_owned(),
            external_bin: config.external_bin.clone(),
        },
        plugins: TauriPluginsConfig {
            deep_link: DeepLinkPluginConfig {
                desktop: DeepLinkDesktopConfig {
                    schemes: vec![config.scheme.clone()],
                },
            },
        },
    }
}

pub(crate) fn build_default_capability_document(scheme: &str) -> TauriCapabilityDocument {
    TauriCapabilityDocument {
        schema: "../gen/schemas/desktop-schema.json".to_owned(),
        identifier: "default".to_owned(),
        description: "Default LegionCode desktop capability for the main application window."
            .to_owned(),
        windows: vec!["main".to_owned()],
        remote: TauriRemoteUrls {
            urls: vec![format!("{scheme}://{DESKTOP_HOST}/*")],
        },
        permissions: vec![
            "core:default".to_owned(),
            "core:window:allow-set-title".to_owned(),
            "core:window:allow-set-size".to_owned(),
            "core:window:allow-set-position".to_owned(),
            "core:window:allow-maximize".to_owned(),
            "core:window:allow-unmaximize".to_owned(),
            "core:window:allow-minimize".to_owned(),
            "core:window:allow-show".to_owned(),
            "core:window:allow-hide".to_owned(),
            "core:window:allow-close".to_owned(),
            "core:window:allow-start-dragging".to_owned(),
            "core:event:default".to_owned(),
            "shell:allow-spawn".to_owned(),
        ],
    }
}

pub(crate) fn render_aur_pkgbuild(
    version: &str,
    channel: DesktopUpdateChannel,
    aur: &AurLauncherBundle,
) -> String {
    let brand = legion_contracts::brand::ACTIVE_BRAND;
    let slug = brand.url_scheme;
    let bin = brand.cli_bin_name;
    let nightly_slug = format!("{slug}-nightly");
    let pkgver = version.replace('-', "_");
    let (pkgdesc, provides_name, conflicts_line, desktop_symlink) = match channel {
        DesktopUpdateChannel::Latest => (
            "Desktop control surface for local coding agents".to_owned(),
            slug.to_owned(),
            format!("conflicts=('{slug}')"),
            brand.desktop_bin_name.to_owned(),
        ),
        DesktopUpdateChannel::Nightly => (
            "Nightly desktop control surface for local coding agents".to_owned(),
            nightly_slug.clone(),
            format!("conflicts=('{nightly_slug}' '{slug}')"),
            format!("{bin}-nightly-desktop"),
        ),
    };
    let bin_basename = aur
        .launcher_path
        .rsplit('/')
        .next()
        .unwrap_or(&provides_name);

    [
        format!("pkgname={}", aur.package_name),
        format!("pkgver={pkgver}"),
        "pkgrel=1".to_owned(),
        format!("pkgdesc='{pkgdesc}'"),
        "arch=('x86_64' 'aarch64')".to_owned(),
        format!("url='https://github.com/9thLevelSoftware/{slug}'"),
        "license=('MIT')".to_owned(),
        "depends=('glibc' 'gcc-libs' 'gtk3' 'webkit2gtk-4.1' 'hicolor-icon-theme' 'xdg-utils')"
            .to_owned(),
        format!("provides=(\"{provides_name}=$pkgver\")"),
        conflicts_line,
        "options=('!debug' '!strip')".to_owned(),
        String::new(),
        "package() {".to_owned(),
        format!("  install -d \"$pkgdir{}\"", aur.appdir),
        format!(
            "  install -Dm755 /dev/stdin \"$pkgdir{}\" <<'EOF'",
            aur.launcher_path
        ),
        aur.launcher_script.trim_end_matches('\n').to_owned(),
        "EOF".to_owned(),
        format!("  ln -s {bin_basename} \"$pkgdir/usr/bin/{desktop_symlink}\""),
        format!(
            "  install -Dm644 /dev/stdin \"$pkgdir/usr/share/applications/{}\" <<'EOF'",
            aur.desktop_entry_id
        ),
        aur.desktop_entry.trim_end_matches('\n').to_owned(),
        "EOF".to_owned(),
        "}".to_owned(),
        String::new(),
    ]
    .join("\n")
}

pub(crate) fn ordered_bundle_files(
    a: &crate::bundle::DesktopBundleArtifacts,
) -> Vec<(&'static str, &'static str, Vec<u8>)> {
    vec![
        (
            "tauri.conf.json",
            "utf-8",
            a.tauri_conf_json.as_bytes().to_vec(),
        ),
        (
            "capabilities/default.json",
            "utf-8",
            a.capabilities_default_json.as_bytes().to_vec(),
        ),
        (
            "linux/legioncode.desktop",
            "utf-8",
            a.linux_desktop_entry.as_bytes().to_vec(),
        ),
        (
            "linux/PKGBUILD",
            "utf-8",
            a.linux_pkgbuild.as_bytes().to_vec(),
        ),
        (
            "linux/legioncode-launcher.sh",
            "utf-8",
            a.linux_launcher_script.as_bytes().to_vec(),
        ),
        (
            "services/legioncode.service",
            "utf-8",
            a.systemd_unit.as_bytes().to_vec(),
        ),
        (
            "services/dev.legioncode.service.plist",
            "utf-8",
            a.launchd_plist.as_bytes().to_vec(),
        ),
        (
            "services/legioncode-task.xml",
            "utf-16le-bom",
            a.windows_task_utf16le_bom.clone(),
        ),
    ]
}
