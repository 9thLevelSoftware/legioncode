//! AUR launcher script and XDG `.desktop` entry generation (`BUG-31`).

use serde::{Deserialize, Serialize};
use t3_contracts::brand::{ACTIVE_BRAND, BrandConfig};

/// Desktop release channel (`packages/contracts/src/ipc.ts:88`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DesktopUpdateChannel {
    /// Stable release track (`latest`).
    Latest,
    /// Nightly prerelease track (`nightly`).
    Nightly,
}

/// Generated AUR wrapper launcher script and XDG `.desktop` entry (`BUG-31`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AurLauncherBundle {
    /// AUR package name (e.g. `t3code-bin` or `t3code-nightly-bin`).
    pub package_name: String,
    /// Installed wrapper script path (e.g. `/usr/bin/t3code` or `/usr/bin/t3code-nightly`).
    pub launcher_path: String,
    /// Extracted `AppImage` directory (e.g. `/opt/t3code-bin` or `/opt/t3code-nightly-bin`).
    pub appdir: String,
    /// XDG desktop file basename (e.g. `com.t3tools.T3Code.desktop` or `com.t3tools.T3CodeNightly.desktop`).
    pub desktop_entry_id: String,
    /// `StartupWMClass` matching the desktop entry ID without `.desktop`.
    pub startup_wm_class: String,
    /// POSIX `/bin/sh` wrapper script setting `APPDIR` and `<BRAND>_DESKTOP_LAUNCHER_PATH`.
    pub launcher_script: String,
    /// Freedesktop `.desktop` entry contents.
    pub desktop_entry: String,
}

fn desktop_app_prefix(brand: &BrandConfig) -> String {
    let base = brand
        .launchd_label
        .strip_suffix(".service")
        .unwrap_or(brand.launchd_label);
    let scheme_dot = format!(".{}", brand.url_scheme);
    let domain_prefix = match base.strip_suffix(&scheme_dot) {
        Some(stripped) if stripped.contains('.') => stripped,
        _ => base,
    };
    let compact_name: String = brand.product_name.split_whitespace().collect();
    format!("{domain_prefix}.{compact_name}")
}

/// Renders the AUR wrapper script and XDG `.desktop` entry for `channel` using [`ACTIVE_BRAND`] (`BUG-31`).
#[must_use]
pub fn render_aur_launcher(channel: DesktopUpdateChannel) -> AurLauncherBundle {
    render_aur_launcher_for_brand(channel, &ACTIVE_BRAND)
}

/// Renders the AUR wrapper script and XDG `.desktop` entry for `channel` and `brand` (`BUG-31`).
///
/// Emits `<PRIMARY_ENV_PREFIX>DESKTOP_LAUNCHER_PATH=<launcher_path>` alongside `APPDIR=<appdir>`
/// so Electron's URL-handler and `AppImage` environment-scrub paths stay active across `.desktop`
/// and `<url_scheme>://` launches, and assigns distinct Nightly desktop IDs and `StartupWMClass`
/// values to avoid colliding with stable installs.
#[must_use]
pub fn render_aur_launcher_for_brand(
    channel: DesktopUpdateChannel,
    brand: &BrandConfig,
) -> AurLauncherBundle {
    let slug = brand.url_scheme;
    let launcher_env = format!("{}DESKTOP_LAUNCHER_PATH", brand.primary_env_prefix);
    let wm_base = desktop_app_prefix(brand);
    let (package_name, bin_name, icon_name, startup_wm_class, display_name, comment) = match channel
    {
        DesktopUpdateChannel::Latest => (
            format!("{slug}-bin"),
            slug.to_owned(),
            slug.to_owned(),
            wm_base,
            brand.product_name.to_owned(),
            "Desktop control surface for local coding agents",
        ),
        DesktopUpdateChannel::Nightly => (
            format!("{slug}-nightly-bin"),
            format!("{slug}-nightly"),
            format!("{slug}-nightly"),
            format!("{wm_base}Nightly"),
            format!("{} Nightly", brand.product_name),
            "Nightly desktop control surface for local coding agents",
        ),
    };
    let launcher_path = format!("/usr/bin/{bin_name}");
    let appdir = format!("/opt/{package_name}");
    let desktop_entry_id = format!("{startup_wm_class}.desktop");

    let launcher_script = format!(
        "#!/bin/sh\n\
         # AppRun computes APPDIR but doesn't export it; {launcher_env} ensures\n\
         # .desktop and {slug}:// launches re-enter this scrubbed wrapper instead of AppRun.\n\
         exec env APPDIR={appdir} {launcher_env}={launcher_path} {appdir}/AppRun \"$@\"\n",
    );

    let desktop_entry = format!(
        "[Desktop Entry]\n\
         Name={display_name}\n\
         Comment={comment}\n\
         Exec={bin_name} %U\n\
         TryExec={bin_name}\n\
         Terminal=false\n\
         Type=Application\n\
         Icon={icon_name}\n\
         StartupWMClass={startup_wm_class}\n\
         Categories=Development;\n\
         MimeType=x-scheme-handler/{slug};\n",
    );

    AurLauncherBundle {
        package_name,
        launcher_path,
        appdir,
        desktop_entry_id,
        startup_wm_class,
        launcher_script,
        desktop_entry,
    }
}
