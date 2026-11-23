//! Prunes unused pinned runtimes and abandoned `.staging-*` directories (`#40817`).
//!
//! Mirrors `prunePinnedRuntimes` in `apps/server/src/cloud/pinnedRuntime.ts`.

use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::io::ErrorKind;
use std::path::{MAIN_SEPARATOR, Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use super::state::ServiceUpdateRecord;
use super::{SERVICE_STATE_FILE, is_exact_service_version, parse_service_state};

/// Grace period (1 hour) before newly installed or staged runtimes become eligible for pruning.
pub const PINNED_RUNTIME_PRUNE_GRACE_MS: u128 = 3_600_000;

const PINNED_RUNTIME_DIR: &str = "runtime";
const PINNED_RUNTIME_VERSIONS_DIR: &str = "versions";
const PINNED_RUNTIME_LAUNCHERS_FILE: &str = "launchers";
const PINNED_RUNTIME_STAGING_PREFIX: &str = ".staging-";
const PINNED_RUNTIME_PRUNED_PREFIX: &str = ".pruned-";
const INSTALL_SENTINEL_FILE: &str = ".install-complete";

/// Removes unused pinned runtimes and abandoned staging directories under `<base_dir>/runtime/versions`.
///
/// Keeps versions listed in `keep`, referenced by `service-state.json` (except `from_version` on a
/// committed update), currently running processes, recorded shell launchers, or `references`, plus
/// any install or staging directory touched within [`PINNED_RUNTIME_PRUNE_GRACE_MS`]. Returns the
/// version strings of removed completed runtimes (excluding `.staging-*` directories).
pub fn prune_pinned_runtimes(base_dir: &Path, keep: &[&str], references: &[&str]) -> Vec<String> {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let running = collect_running_executables();
    prune_pinned_runtimes_at(base_dir, keep, references, now_ms, running.as_deref())
}

/// Deterministic variant of [`prune_pinned_runtimes`] accepting an explicit wall-clock timestamp and
/// process listing for unit testing.
pub fn prune_pinned_runtimes_at(
    base_dir: &Path,
    keep: &[&str],
    references: &[&str],
    now_ms: u128,
    running_processes: Option<&str>,
) -> Vec<String> {
    let pruned = select_unused_runtimes(base_dir, keep, references, now_ms, running_processes);
    let mut removed = Vec::new();
    for target in pruned {
        let Some(name) = target
            .file_name()
            .and_then(|n| n.to_str())
            .map(str::to_owned)
        else {
            continue;
        };
        if fs::remove_dir_all(&target).is_ok()
            && let Some(version) = name.strip_prefix(PINNED_RUNTIME_PRUNED_PREFIX)
        {
            removed.push(version.to_owned());
        }
    }
    removed
}

fn select_unused_runtimes(
    base_dir: &Path,
    keep: &[&str],
    references: &[&str],
    now_ms: u128,
    running_processes: Option<&str>,
) -> BTreeSet<PathBuf> {
    let versions_dir = base_dir
        .join(PINNED_RUNTIME_DIR)
        .join(PINNED_RUNTIME_VERSIONS_DIR);
    let Ok(read_dir) = fs::read_dir(&versions_dir) else {
        return BTreeSet::new();
    };

    let mut entry_names = Vec::new();
    for entry in read_dir.flatten() {
        if let Some(name) = entry.file_name().to_str() {
            entry_names.push(name.to_owned());
        }
    }

    // Interrupted prunes already renamed their target directory to `.pruned-*`.
    let mut pruned: BTreeSet<PathBuf> = entry_names
        .iter()
        .filter(|name| name.starts_with(PINNED_RUNTIME_PRUNED_PREFIX))
        .map(|name| versions_dir.join(name))
        .collect();

    let settled_before = now_ms.saturating_sub(PINNED_RUNTIME_PRUNE_GRACE_MS);
    let mut completed = Vec::new();
    for name in &entry_names {
        if name.starts_with(PINNED_RUNTIME_STAGING_PREFIX) {
            // A crashed or interrupted install leaves its `.staging-*` directory behind (#40817).
            let staging_dir = versions_dir.join(name);
            if let Ok(meta) = fs::symlink_metadata(&staging_dir)
                && !meta.file_type().is_symlink()
                && modified_epoch_ms(&meta).is_some_and(|mtime| mtime < settled_before)
            {
                pruned.insert(staging_dir);
            }
            continue;
        }
        if !is_exact_service_version(name) {
            continue;
        }
        let target_dir = versions_dir.join(name);
        let Ok(dir_meta) = fs::symlink_metadata(&target_dir) else {
            continue;
        };
        if dir_meta.file_type().is_symlink() {
            continue;
        }
        let sentinel_path = target_dir.join(INSTALL_SENTINEL_FILE);
        let Ok(sentinel_text) = fs::read_to_string(&sentinel_path) else {
            continue;
        };
        if sentinel_text.trim() != name {
            continue;
        }
        let Ok(sentinel_meta) = fs::metadata(&sentinel_path) else {
            continue;
        };
        if modified_epoch_ms(&sentinel_meta).is_some_and(|mtime| mtime < settled_before) {
            completed.push(name.clone());
        }
    }

    if completed.is_empty() {
        return pruned;
    }

    let mut keep_set: HashSet<String> = keep.iter().map(|v| (*v).to_owned()).collect();
    if !collect_state_keeps(base_dir, &mut keep_set) {
        return pruned;
    }

    // Without a process list, a runtime currently in use would appear unused.
    let Some(processes) = running_processes else {
        return pruned;
    };

    let launcher_targets = read_recorded_launcher_targets(base_dir);
    let haystack: Vec<String> = std::iter::once(fold_case(processes))
        .chain(launcher_targets.into_iter().map(|s| fold_case(&s)))
        .chain(references.iter().map(|s| fold_case(s)))
        .collect();

    let real_versions_dir =
        fs::canonicalize(&versions_dir).unwrap_or_else(|_| versions_dir.clone());

    for version in completed {
        if keep_set.contains(&version) {
            continue;
        }
        let candidate_dir = versions_dir.join(&version);
        let prefixes = [
            folded_dir_prefix(&candidate_dir),
            folded_dir_prefix(&real_versions_dir.join(&version)),
        ];
        if haystack
            .iter()
            .any(|entry| prefixes.iter().any(|prefix| entry.contains(prefix)))
        {
            continue;
        }
        let target = versions_dir.join(format!("{PINNED_RUNTIME_PRUNED_PREFIX}{version}"));
        let _ = fs::remove_dir_all(&target);
        if fs::rename(&candidate_dir, &target).is_ok() {
            pruned.insert(target);
        }
    }

    pruned
}

fn collect_state_keeps(base_dir: &Path, keep_set: &mut HashSet<String>) -> bool {
    let state_path = base_dir.join(PINNED_RUNTIME_DIR).join(SERVICE_STATE_FILE);
    let state_text = match fs::read_to_string(&state_path) {
        Ok(text) => text,
        Err(err) if err.kind() == ErrorKind::NotFound => return true,
        Err(_) => return false,
    };
    let Some(state) = parse_service_state(&state_text) else {
        return false;
    };
    keep_set.insert(state.active_version);
    if let Some(update) = state.update {
        // A committed update never returns to the runtime it replaced (#40817).
        if !matches!(update, ServiceUpdateRecord::Committed { .. }) {
            keep_set.insert(update.baseline_version().to_owned());
        }
        keep_set.insert(update.target_version().to_owned());
    }
    true
}

fn read_recorded_launcher_targets(base_dir: &Path) -> Vec<String> {
    let launchers_path = base_dir
        .join(PINNED_RUNTIME_DIR)
        .join(PINNED_RUNTIME_LAUNCHERS_FILE);
    let Ok(contents) = fs::read_to_string(launchers_path) else {
        return Vec::new();
    };
    let mut targets = Vec::new();
    for line in contents.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let launcher_path = Path::new(line);
        if let Ok(resolved) = fs::canonicalize(launcher_path) {
            targets.push(strip_verbatim_prefix(&resolved.to_string_lossy()));
        } else {
            targets.push(line.to_owned());
        }
    }
    targets
}

fn folded_dir_prefix(dir: &Path) -> String {
    let raw = strip_verbatim_prefix(&dir.to_string_lossy());
    let mut out = fold_case(&raw);
    if !out.ends_with(MAIN_SEPARATOR) {
        out.push(MAIN_SEPARATOR);
    }
    out
}

fn strip_verbatim_prefix(path: &str) -> String {
    if cfg!(windows) {
        path.strip_prefix(r"\\?\").unwrap_or(path).to_owned()
    } else {
        path.to_owned()
    }
}

fn fold_case(value: &str) -> String {
    if cfg!(windows) {
        value.to_ascii_lowercase()
    } else {
        value.to_owned()
    }
}

fn modified_epoch_ms(meta: &fs::Metadata) -> Option<u128> {
    meta.modified()
        .ok()
        .and_then(|mtime| mtime.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis())
}

fn collect_running_executables() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let entries = fs::read_dir("/proc").ok()?;
        let mut executables = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(pid) = name.to_str() else {
                continue;
            };
            if !pid.bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            if let Ok(target) = fs::read_link(entry.path().join("exe")) {
                executables.push(target.to_string_lossy().into_owned());
            }
        }
        if executables.is_empty() {
            None
        } else {
            Some(executables.join("\n"))
        }
    }
    #[cfg(windows)]
    {
        let output = Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); Get-CimInstance Win32_Process | Select-Object -ExpandProperty CommandLine",
            ])
            .output()
            .ok()?;
        if !output.status.success() || output.stdout.is_empty() {
            return None;
        }
        String::from_utf8(output.stdout).ok()
    }
    #[cfg(all(unix, not(target_os = "linux")))]
    {
        let output = Command::new("lsof")
            .args(["-nP", "-w", "-d", "txt", "-F", "n"])
            .output()
            .ok()?;
        if !output.status.success() || output.stdout.is_empty() {
            return None;
        }
        String::from_utf8(output.stdout).ok()
    }
}
