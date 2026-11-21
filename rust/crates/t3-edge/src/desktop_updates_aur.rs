//! AUR launcher script and XDG `.desktop` entry generation (`BUG-31`).

use serde::{Deserialize, Serialize};

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
    /// AUR package name (`t3code-bin` or `t3code-nightly-bin`).
    pub package_name: &'static str,
    /// Installed wrapper script path (`/usr/bin/t3code` or `/usr/bin/t3code-nightly`).
    pub launcher_path: &'static str,
    /// Extracted `AppImage` directory (`/opt/t3code-bin` or `/opt/t3code-nightly-bin`).
    pub appdir: &'static str,
    /// XDG desktop file basename (`com.t3tools.T3Code.desktop` or `com.t3tools.T3CodeNightly.desktop`).
    pub desktop_entry_id: &'static str,
    /// `StartupWMClass` matching the desktop entry ID without `.desktop`.
    pub startup_wm_class: &'static str,
    /// POSIX `/bin/sh` wrapper script setting `APPDIR` and `T3CODE_DESKTOP_LAUNCHER_PATH`.
    pub launcher_script: String,
    /// Freedesktop `.desktop` entry contents.
    pub desktop_entry: String,
}

struct AurSpec {
    package_name: &'static str,
    bin_name: &'static str,
    launcher_path: &'static str,
    appdir: &'static str,
    desktop_entry_id: &'static str,
    startup_wm_class: &'static str,
    display_name: &'static str,
    comment: &'static str,
    icon_name: &'static str,
}

const STABLE_AUR_SPEC: AurSpec = AurSpec {
    package_name: "t3code-bin",
    bin_name: "t3code",
    launcher_path: "/usr/bin/t3code",
    appdir: "/opt/t3code-bin",
    desktop_entry_id: "com.t3tools.T3Code.desktop",
    startup_wm_class: "com.t3tools.T3Code",
    display_name: "T3 Code",
    comment: "Desktop control surface for local coding agents",
    icon_name: "t3code",
};

const NIGHTLY_AUR_SPEC: AurSpec = AurSpec {
    package_name: "t3code-nightly-bin",
    bin_name: "t3code-nightly",
    launcher_path: "/usr/bin/t3code-nightly",
    appdir: "/opt/t3code-nightly-bin",
    desktop_entry_id: "com.t3tools.T3CodeNightly.desktop",
    startup_wm_class: "com.t3tools.T3CodeNightly",
    display_name: "T3 Code Nightly",
    comment: "Nightly desktop control surface for local coding agents",
    icon_name: "t3code-nightly",
};

/// Renders the AUR wrapper script and XDG `.desktop` entry for `channel` (`BUG-31`).
///
/// Emits `T3CODE_DESKTOP_LAUNCHER_PATH=<launcher_path>` alongside `APPDIR=<appdir>` so Electron's
/// URL-handler and `AppImage` environment-scrub paths stay active across `.desktop` and `t3code://`
/// launches, and assigns `com.t3tools.T3CodeNightly.desktop` + `StartupWMClass=com.t3tools.T3CodeNightly`
/// on `Nightly` to avoid colliding with stable `com.t3tools.T3Code.desktop`.
pub fn render_aur_launcher(channel: DesktopUpdateChannel) -> AurLauncherBundle {
    let spec = match channel {
        DesktopUpdateChannel::Latest => &STABLE_AUR_SPEC,
        DesktopUpdateChannel::Nightly => &NIGHTLY_AUR_SPEC,
    };

    let launcher_script = format!(
        "#!/bin/sh\n\
         # AppRun computes APPDIR but doesn't export it; T3CODE_DESKTOP_LAUNCHER_PATH ensures\n\
         # .desktop and t3code:// launches re-enter this scrubbed wrapper instead of AppRun.\n\
         exec env APPDIR={} T3CODE_DESKTOP_LAUNCHER_PATH={} {}/AppRun \"$@\"\n",
        spec.appdir, spec.launcher_path, spec.appdir,
    );

    let desktop_entry = format!(
        "[Desktop Entry]\n\
         Name={}\n\
         Comment={}\n\
         Exec={} %U\n\
         TryExec={}\n\
         Terminal=false\n\
         Type=Application\n\
         Icon={}\n\
         StartupWMClass={}\n\
         Categories=Development;\n\
         MimeType=x-scheme-handler/t3code;\n",
        spec.display_name,
        spec.comment,
        spec.bin_name,
        spec.bin_name,
        spec.icon_name,
        spec.startup_wm_class,
    );

    AurLauncherBundle {
        package_name: spec.package_name,
        launcher_path: spec.launcher_path,
        appdir: spec.appdir,
        desktop_entry_id: spec.desktop_entry_id,
        startup_wm_class: spec.startup_wm_class,
        launcher_script,
        desktop_entry,
    }
}
