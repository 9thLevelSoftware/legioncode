//! Browser executable discovery and `PreviewBrowserHost.ts` failure diagnostics.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

const NO_SANDBOX_SIGNATURE: &str = "No usable sandbox";
const LOADER_ERROR_PREFIX: &str = "error while loading shared libraries:";

/// Launch failure classification matching `apps/server/src/preview/PreviewBrowserHost.ts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PreviewLaunchDiagnostic {
    /// Chrome refuses to run its sandbox as UID 0 (`crbug.com/638180`).
    RootSandbox {
        /// Remediation explanation.
        message: String,
    },
    /// Ubuntu 23.10+ `AppArmor` restricts unprivileged user namespaces.
    AppArmorSandbox {
        /// Setup subcommand (`t3 browser setup`).
        setup_command: String,
        /// Remediation explanation.
        message: String,
    },
    /// Linux host is missing dynamic shared libraries required by Chromium.
    MissingLibraries {
        /// Setup subcommand (`t3 browser setup`).
        setup_command: String,
        /// Missing `.so` names detected via loader stderr / `ldd`.
        libraries: Vec<String>,
        /// Remediation explanation.
        message: String,
    },
}

/// Inspect browser launch stderr (`output`) for root/AppArmor/shared-lib failures.
pub fn diagnose_browser_launch_failure(
    executable: &str,
    output: &str,
    setup_command: &str,
    is_root: bool,
    is_linux: bool,
) -> Option<PreviewLaunchDiagnostic> {
    if output.contains(NO_SANDBOX_SIGNATURE) {
        if is_root {
            return Some(PreviewLaunchDiagnostic::RootSandbox {
                message: "T3 server is running as root; Chrome's sandbox cannot start. Run T3 as a nonroot user or set `T3CODE_SERVER_BROWSER_SANDBOX=0`.".to_owned(),
            });
        }
        return Some(PreviewLaunchDiagnostic::AppArmorSandbox {
            setup_command: setup_command.to_owned(),
            message: format!(
                "This host blocks Chrome's sandbox (AppArmor). Run `{setup_command}` on the host once and retry."
            ),
        });
    }
    if is_linux {
        let libs = detect_missing_libraries(executable, output);
        if !libs.is_empty() {
            return Some(PreviewLaunchDiagnostic::MissingLibraries {
                setup_command: setup_command.to_owned(),
                message: format!(
                    "Missing shared libraries ({}). Run `{setup_command}` on the host and retry.",
                    libs.join(", ")
                ),
                libraries: libs,
            });
        }
    }
    None
}

fn detect_missing_libraries(executable: &str, stderr_output: &str) -> Vec<String> {
    let mut libs = Vec::new();
    for line in stderr_output.lines() {
        if let Some(pos) = line.find(LOADER_ERROR_PREFIX) {
            let rest = line[pos + LOADER_ERROR_PREFIX.len()..].trim();
            if let Some(lib) = rest.split([':', ' ']).find(|s| !s.is_empty()) {
                libs.push(lib.to_owned());
            }
        }
    }
    if libs.is_empty()
        && let Ok(out) = Command::new("ldd").arg(executable).output()
    {
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            if let Some(left) = line.strip_suffix("=> not found")
                && let Some(tok) = left.split_whitespace().next()
            {
                libs.push(tok.to_owned());
            }
        }
    }
    libs
}

/// Locate a local Chromium / `chrome-headless-shell` / Edge binary from env or `PATH`.
pub fn discover_browser_executable(explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = explicit
        && p.is_file()
    {
        return Some(p.to_path_buf());
    }
    if let Ok(env_path) = std::env::var("T3CODE_BROWSER_EXECUTABLE") {
        let p = PathBuf::from(env_path.trim());
        if p.is_file() {
            return Some(p);
        }
    }
    let candidates = [
        "chrome-headless-shell",
        "chromium",
        "chromium-browser",
        "google-chrome",
        "msedge",
    ];
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        for name in candidates {
            let p = dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}
