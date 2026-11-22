//! Atomic theme staging, rollback backup, and revert handle.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use uuid::Uuid;

use crate::theme::{
    MAX_THEME_FILE_BYTES, ThemeError, is_unpublishable_theme_id, is_valid_theme_id,
};

/// Handle returned by [`publish_theme_file`] supporting rollback if a downstream settings write fails.
#[derive(Debug)]
pub struct PublishedThemeHandle {
    destination: PathBuf,
    backup_path: Option<PathBuf>,
    fingerprint: Option<FileFingerprint>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileFingerprint {
    #[cfg(unix)]
    dev: u64,
    #[cfg(unix)]
    ino: u64,
    len: u64,
    modified: Option<std::time::SystemTime>,
}

impl FileFingerprint {
    fn from_metadata(meta: &fs::Metadata) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Self {
                dev: meta.dev(),
                ino: meta.ino(),
                len: meta.len(),
                modified: meta.modified().ok(),
            }
        }
        #[cfg(not(unix))]
        {
            Self {
                len: meta.len(),
                modified: meta.modified().ok(),
            }
        }
    }
}

impl PublishedThemeHandle {
    /// Reverts the published theme back to the pre-existing file (or removes it when newly created).
    ///
    /// If the destination was overwritten concurrently since [`publish_theme_file`] completed, the newer
    /// file is preserved and only the rollback backup is cleaned up.
    ///
    /// # Errors
    ///
    /// Returns [`ThemeError::PublishIo`] if renaming the backup or removing the destination fails.
    pub fn revert(self) -> Result<(), ThemeError> {
        let same_destination = match (fs::symlink_metadata(&self.destination), self.fingerprint) {
            (Ok(meta), Some(expected)) => FileFingerprint::from_metadata(&meta) == expected,
            (Err(err), _) if err.kind() == std::io::ErrorKind::NotFound => false,
            _ => false,
        };

        if !same_destination {
            return self.cleanup();
        }

        if let Some(backup) = &self.backup_path {
            fs::rename(backup, &self.destination).map_err(|source| ThemeError::PublishIo {
                path: self.destination.clone(),
                source,
            })?;
        } else {
            match fs::remove_file(&self.destination) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(source) => {
                    return Err(ThemeError::PublishIo {
                        path: self.destination,
                        source,
                    });
                }
            }
        }
        Ok(())
    }

    /// Deletes the rollback backup once `settings.json` has been updated successfully.
    ///
    /// # Errors
    ///
    /// Returns [`ThemeError::PublishIo`] if deleting the rollback backup fails for reasons other than `NotFound`.
    pub fn cleanup(self) -> Result<(), ThemeError> {
        if let Some(backup) = self.backup_path {
            match fs::remove_file(&backup) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(source) => {
                    return Err(ThemeError::PublishIo {
                        path: backup,
                        source,
                    });
                }
            }
        }
        Ok(())
    }
}

/// Atomically stages and publishes `raw_bytes` to `<themes_dir>/<theme_id>.json`, returning a rollback handle.
///
/// # Errors
///
/// Returns [`ThemeError`] if `theme_id` is invalid/unpublishable, `raw_bytes` exceeds [`MAX_THEME_FILE_BYTES`],
/// the destination is a directory, or staging/renaming fails.
pub fn publish_theme_file(
    themes_dir: &Path,
    theme_id: &str,
    raw_bytes: &[u8],
) -> Result<PublishedThemeHandle, ThemeError> {
    if !is_valid_theme_id(theme_id) {
        return Err(ThemeError::InvalidId {
            id: theme_id.to_owned(),
        });
    }
    if is_unpublishable_theme_id(theme_id) {
        return Err(ThemeError::UnpublishableId {
            id: theme_id.to_owned(),
        });
    }
    if raw_bytes.len() > MAX_THEME_FILE_BYTES {
        return Err(ThemeError::FileTooLarge {
            path: themes_dir.join(format!("{theme_id}.json")),
            max_bytes: MAX_THEME_FILE_BYTES,
        });
    }

    fs::create_dir_all(themes_dir).map_err(|source| ThemeError::PublishIo {
        path: themes_dir.to_path_buf(),
        source,
    })?;

    let destination = themes_dir.join(format!("{theme_id}.json"));
    if let Ok(meta) = fs::symlink_metadata(&destination)
        && meta.is_dir()
    {
        return Err(ThemeError::DestinationDirectory { path: destination });
    }

    let staging = themes_dir.join(format!("{theme_id}.json.staging-{}", std::process::id()));
    let _ = fs::remove_file(&staging);
    write_new_file_with_mode(&staging, raw_bytes, 0o644)?;

    let rollback_path = match fs::symlink_metadata(&destination) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(source) => {
            let _ = fs::remove_file(&staging);
            return Err(ThemeError::PublishIo {
                path: destination,
                source,
            });
        }
        Ok(meta) => {
            if meta.is_dir() {
                let _ = fs::remove_file(&staging);
                return Err(ThemeError::DestinationDirectory { path: destination });
            }
            let backup = themes_dir.join(format!("{theme_id}.json.rollback-{}", Uuid::new_v4()));
            if let Err(err) = write_new_file_with_mode(&backup, &[], 0o600) {
                let _ = fs::remove_file(&staging);
                return Err(err);
            }
            match fs::rename(&destination, &backup) {
                Ok(()) => Some(backup),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    let _ = fs::remove_file(&backup);
                    None
                }
                Err(source) => {
                    let _ = fs::remove_file(&backup);
                    let _ = fs::remove_file(&staging);
                    return Err(ThemeError::PublishIo {
                        path: destination,
                        source,
                    });
                }
            }
        }
    };

    if let Err(source) = fs::rename(&staging, &destination) {
        let _ = fs::remove_file(&staging);
        if let Some(backup) = &rollback_path {
            let _ = fs::rename(backup, &destination);
        }
        return Err(ThemeError::PublishIo {
            path: destination,
            source,
        });
    }

    let fingerprint = fs::symlink_metadata(&destination)
        .ok()
        .map(|meta| FileFingerprint::from_metadata(&meta));

    Ok(PublishedThemeHandle {
        destination,
        backup_path: rollback_path,
        fingerprint,
    })
}

fn write_new_file_with_mode(path: &Path, bytes: &[u8], mode: u32) -> Result<(), ThemeError> {
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(mode);
    }
    #[cfg(not(unix))]
    let _ = mode;
    let mut file = opts.open(path).map_err(|source| ThemeError::PublishIo {
        path: path.to_path_buf(),
        source,
    })?;
    file.write_all(bytes)
        .map_err(|source| ThemeError::PublishIo {
            path: path.to_path_buf(),
            source,
        })?;
    file.sync_all().map_err(|source| ThemeError::PublishIo {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(())
}
