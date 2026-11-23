//! Symlink-safe local file cleanup for rotated logs and browser artifacts (`#40528`).
//!
//! Mirrors `cleanFiles` in `apps/server/src/storageCleanup.ts`.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::time::{Duration, SystemTime};

use super::MaintenanceError;

/// Summary of expired local files removed by [`clean_expired_storage_files`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StorageFileCleanupResult {
    /// Count of expired files unlinked.
    pub files: u64,
    /// Total uncompressed bytes of removed files.
    pub bytes: u64,
}

/// Removes expired regular files under `root` older than `max_age` without following symlinks (`#40528`).
///
/// When `rotated_logs_only` is `true`, descends into non-symlink subdirectories and deletes only
/// rotated log files matching `*.log.<digits>` or `*.ndjson.<digits>`. When `false`, inspects
/// top-level regular files (matching `browser-artifacts`). Missing `root` returns zeroes.
pub fn clean_expired_storage_files(
    root: &Path,
    max_age: Duration,
    rotated_logs_only: bool,
    now: SystemTime,
) -> Result<StorageFileCleanupResult, MaintenanceError> {
    clean_expired_storage_files_with_hook(root, max_age, rotated_logs_only, now, &mut |_| {})
}

/// Internal entry point allowing tests to simulate TOCTOU symlink swaps during directory traversal.
pub fn clean_expired_storage_files_with_hook(
    root: &Path,
    max_age: Duration,
    rotated_logs_only: bool,
    now: SystemTime,
    on_before_read_dir: &mut dyn FnMut(&Path),
) -> Result<StorageFileCleanupResult, MaintenanceError> {
    let root_meta = match fs::symlink_metadata(root) {
        Ok(meta) => meta,
        Err(err) if err.kind() == ErrorKind::NotFound => {
            return Ok(StorageFileCleanupResult::default());
        }
        Err(err) => {
            return Err(MaintenanceError::StorageCleanup {
                path: root.to_path_buf(),
                message: err.to_string(),
            });
        }
    };
    if root_meta.file_type().is_symlink() || !root_meta.is_dir() {
        return Ok(StorageFileCleanupResult::default());
    }
    let real_root = fs::canonicalize(root).map_err(|err| MaintenanceError::StorageCleanup {
        path: root.to_path_buf(),
        message: err.to_string(),
    })?;
    let mut result = StorageFileCleanupResult::default();
    visit_dir(
        &real_root,
        &real_root,
        max_age,
        rotated_logs_only,
        now,
        on_before_read_dir,
        &mut result,
    )?;
    Ok(result)
}

fn visit_dir(
    real_root: &Path,
    directory: &Path,
    max_age: Duration,
    rotated_logs_only: bool,
    now: SystemTime,
    on_before_read_dir: &mut dyn FnMut(&Path),
    result: &mut StorageFileCleanupResult,
) -> Result<(), MaintenanceError> {
    on_before_read_dir(directory);
    let Ok(dir_meta) = fs::symlink_metadata(directory) else {
        return Ok(());
    };
    if dir_meta.file_type().is_symlink() || !dir_meta.is_dir() {
        return Ok(());
    }
    let Ok(real_dir) = fs::canonicalize(directory) else {
        return Ok(());
    };
    if real_dir != directory || !real_dir.starts_with(real_root) {
        return Ok(());
    }

    let entries = fs::read_dir(directory).map_err(|err| MaintenanceError::StorageCleanup {
        path: directory.to_path_buf(),
        message: err.to_string(),
    })?;
    for entry_res in entries {
        let entry = entry_res.map_err(|err| MaintenanceError::StorageCleanup {
            path: directory.to_path_buf(),
            message: err.to_string(),
        })?;
        let name = entry.file_name();
        let target = directory.join(&name);
        if !target.starts_with(real_root) {
            continue;
        }
        // Check symlink_metadata + canonicalize before touching any entry (#40528).
        let Ok(meta) = fs::symlink_metadata(&target) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        let Ok(real_target) = fs::canonicalize(&target) else {
            continue;
        };
        if real_target != target {
            continue;
        }
        if meta.is_dir() && rotated_logs_only {
            visit_dir(
                real_root,
                &target,
                max_age,
                rotated_logs_only,
                now,
                on_before_read_dir,
                result,
            )?;
        } else if meta.is_file()
            && (!rotated_logs_only || name.to_str().is_some_and(is_rotated_log_filename))
            && let Ok(modified) = meta.modified()
            && now.duration_since(modified).is_ok_and(|age| age > max_age)
        {
            let size = meta.len();
            if fs::remove_file(&target).is_ok() {
                result.files += 1;
                result.bytes += size;
            }
        }
    }
    Ok(())
}

fn is_rotated_log_filename(name: &str) -> bool {
    let Some((stem, suffix)) = name.rsplit_once('.') else {
        return false;
    };
    !suffix.is_empty()
        && suffix.bytes().all(|b| b.is_ascii_digit())
        && Path::new(stem)
            .extension()
            .is_some_and(|ext| ext == "log" || ext == "ndjson")
}
