//! Launcher shim repointing across `PATH`, `INSTALL_BIN_DIR`, and `<parent_of_versions_dir>/launchers` (`#40309`).

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use legion_contracts::brand::ACTIVE_BRAND;

use crate::cmd_install::{extract_windows_shim_target, write_launcher_file};
use crate::fallback::resolve_platform_path;

/// Repoints recorded and discovered CLI launchers under `versions_dir`'s parent `launchers` file
/// (including Windows `.cmd` shims outside `PATH`), `INSTALL_BIN_DIR`, and `PATH`.
pub(crate) fn repoint_recorded_launchers_for_versions(versions_dir: &Path, new_entry: &Path) {
    let Some(runtime_dir) = versions_dir.parent() else {
        return;
    };
    let launchers_file = runtime_dir.join("launchers");
    let mut seen: Vec<PathBuf> = Vec::new();

    if let Ok(contents) = fs::read_to_string(&launchers_file) {
        for line in contents.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let p = PathBuf::from(line);
            if p.is_absolute() && !seen.iter().any(|existing| existing == &p) {
                seen.push(p);
            }
        }
    }

    let cmd_name = format!("{}.cmd", ACTIVE_BRAND.cli_bin_name);
    if let Some(install_bin) = ACTIVE_BRAND.read_env("INSTALL_BIN_DIR") {
        let cand = PathBuf::from(install_bin.trim()).join(&cmd_name);
        if !seen.iter().any(|existing| existing == &cand) {
            seen.push(cand);
        }
    }

    if cfg!(windows)
        && let Ok(path_env) = env::var("PATH")
    {
        for dir in path_env.split(';').map(str::trim).filter(|d| !d.is_empty()) {
            let cand = PathBuf::from(dir).join(&cmd_name);
            if !seen.iter().any(|existing| existing == &cand) {
                seen.push(cand);
            }
        }
    }

    let abs_versions = resolve_platform_path(versions_dir);
    for launcher in seen {
        if !launcher.is_file() {
            continue;
        }
        let is_recorded_cmd = launcher
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.eq_ignore_ascii_case(&cmd_name));
        let should_repoint = if is_recorded_cmd && cfg!(windows) {
            fs::read_to_string(&launcher)
                .ok()
                .and_then(|raw| extract_windows_shim_target(&raw).map(str::to_owned))
                .is_some_and(|target| {
                    resolve_platform_path(Path::new(&target)).starts_with(&abs_versions)
                })
        } else {
            true
        };
        if should_repoint {
            let _ = write_launcher_file(&launcher, new_entry);
        }
    }
}
