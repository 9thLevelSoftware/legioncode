//! Native OS file/folder pickers, safe external URL launcher, locale detection,
//! remote editor PATH probing, and Windows/WSL backend path translation.

use std::env;
use std::path::PathBuf;
use std::process::Command;

use crate::http_loopback::is_valid_external_url;
pub use crate::os_pickers::{
    MAX_THEME_FILE_BYTES, PickedThemeFile, pick_folder, pick_project_favicon, pick_theme_files,
    read_picked_theme_files,
};

/// Returns the client platform string matching Node `process.platform` (`win32`, `darwin`, `linux`).
#[must_use]
pub const fn client_platform() -> &'static str {
    if cfg!(target_os = "windows") {
        "win32"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else {
        "linux"
    }
}

/// Resolves preferred OS BCP-47 language tags from POSIX locale environment variables.
#[must_use]
pub fn detect_preferred_system_languages() -> Vec<String> {
    let mut tags = Vec::new();
    for key in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(raw) = env::var(key)
            && let Some(tag) = normalize_posix_locale(&raw)
            && !tags.contains(&tag)
        {
            tags.push(tag);
        }
    }
    if let Ok(raw_list) = env::var("LANGUAGE") {
        for part in raw_list.split(':') {
            if let Some(tag) = normalize_posix_locale(part)
                && !tags.contains(&tag)
            {
                tags.push(tag);
            }
        }
    }
    if tags.is_empty() {
        tags.push("en-US".to_owned());
    }
    tags
}

/// Normalizes POSIX locale identifiers (e.g. `en_US.UTF-8` -> `en-US`, ignoring `C`/`POSIX`).
#[must_use]
pub fn normalize_posix_locale(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let without_codeset = trimmed.split(['.', '@']).next()?.trim();
    if without_codeset.eq_ignore_ascii_case("C") || without_codeset.eq_ignore_ascii_case("POSIX") {
        return None;
    }
    Some(without_codeset.replace('_', "-"))
}

/// Translates a Windows host path into a path visible inside `backend_id` (`primary` or `wsl:<distro>`).
#[must_use]
pub fn to_local_backend_path(backend_id: &str, windows_path: &str) -> Option<String> {
    let trimmed = windows_path.trim();
    if trimmed.is_empty() {
        return None;
    }
    if backend_id == "primary" || backend_id.is_empty() {
        return Some(trimmed.to_owned());
    }
    if !backend_id.starts_with("wsl") {
        return None;
    }
    if trimmed.starts_with('/') {
        return Some(trimmed.to_owned());
    }
    for prefix in [r"\\wsl.localhost\", r"\\wsl$\"] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            let mut segments = rest.split(['\\', '/']);
            let _distro = segments.next()?;
            let tail: Vec<&str> = segments.filter(|s| !s.is_empty()).collect();
            return Some(format!("/{}", tail.join("/")));
        }
    }
    let bytes = trimmed.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        let drive = (bytes[0] as char).to_ascii_lowercase();
        let after_drive = trimmed[2..].trim_start_matches(['\\', '/']);
        let posix_tail = after_drive.replace('\\', "/");
        if posix_tail.is_empty() {
            return Some(format!("/mnt/{drive}"));
        }
        return Some(format!("/mnt/{drive}/{posix_tail}"));
    }
    None
}

/// Safely opens `url` via the OS shell after verifying `http:`, `https:`, or `mailto:`.
pub fn open_external_url(url: &str) -> Result<bool, String> {
    if !is_valid_external_url(url) {
        return Ok(false);
    }
    let trimmed = url.trim();
    #[cfg(target_os = "windows")]
    let spawn_result = Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", trimmed])
        .spawn();
    #[cfg(target_os = "macos")]
    let spawn_result = Command::new("open").arg(trimmed).spawn();
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    let spawn_result = Command::new("xdg-open").arg(trimmed).spawn();

    match spawn_result {
        Ok(_child) => Ok(true),
        Err(err) => Err(format!("failed opening external URL: {err}")),
    }
}

/// Opens OS system settings for `pane` (currently `"full-disk-access"`).
pub fn open_system_settings(pane: &str) -> bool {
    if pane != "full-disk-access" {
        return false;
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles")
            .spawn()
            .is_ok()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Probes local PATH for remote-capable editor CLIs matching `packages/contracts/src/editor.ts`.
#[must_use]
pub fn probe_remote_editors() -> Vec<String> {
    let path_env = env::var_os("PATH");
    probe_remote_editors_in_path(path_env.as_deref())
}

/// Testable helper scanning remote-capable editor executables in `path_env`.
#[must_use]
pub fn probe_remote_editors_in_path(path_env: Option<&std::ffi::OsStr>) -> Vec<String> {
    const REMOTE_EDITORS: &[(&str, &[&str])] = &[
        ("cursor", &["cursor"]),
        ("vscode", &["code"]),
        ("vscode-insiders", &["code-insiders"]),
        ("vscodium", &["codium"]),
        ("zed", &["zed", "zeditor"]),
    ];
    let dirs: Vec<PathBuf> = path_env.map_or_default(|val| env::split_paths(val).collect());

    let mut found = Vec::new();
    for &(id, cmds) in REMOTE_EDITORS {
        let matched = cmds.iter().any(|cmd| {
            dirs.iter().any(|dir| {
                #[cfg(windows)]
                {
                    ["", ".exe", ".cmd", ".bat"]
                        .iter()
                        .any(|ext| dir.join(format!("{cmd}{ext}")).is_file())
                }
                #[cfg(not(windows))]
                {
                    dir.join(cmd).is_file()
                }
            })
        });
        if matched {
            found.push(id.to_owned());
        }
    }
    found
}
