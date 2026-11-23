//! Attachment upload sweep and Linux btrfs `No_COW` helpers for server directory setup.

use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use uuid::Uuid;

pub(super) struct TempFileGuard {
    pub(super) path: PathBuf,
}

impl TempFileGuard {
    pub(super) fn create(dir: &Path, prefix: &str) -> Result<Self, std::io::Error> {
        let name = format!("{prefix}{}", Uuid::new_v4().simple());
        let path = dir.join(name);
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        drop(file);
        Ok(Self { path })
    }
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub(super) fn recovery_path_for(environment_id_path: &Path) -> PathBuf {
    let mut os = OsString::from(environment_id_path.as_os_str());
    os.push(".recovery");
    PathBuf::from(os)
}

pub(super) fn write_and_sync(path: &Path, value: &str) -> Result<(), std::io::Error> {
    let mut file = OpenOptions::new().write(true).truncate(true).open(path)?;
    writeln!(file, "{value}")?;
    file.sync_all()
}

#[cfg(unix)]
pub(super) fn restrict_state_dir_permissions(state_dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(state_dir, fs::Permissions::from_mode(0o700));
}

#[cfg(not(unix))]
pub(super) fn restrict_state_dir_permissions(_state_dir: &Path) {}

const PENDING_ATTACHMENT_THREAD_SEGMENT: &str = "pending";
const PENDING_ATTACHMENT_MAX_AGE: Duration = Duration::from_hours(24);
const PARTIAL_UPLOAD_MAX_AGE: Duration = Duration::from_hours(1);

/// Deletes expired `.part` and stale `pending-<uuid>` uploads from `attachments_dir`.
///
/// Mirrors `sweepStalePendingAttachments` in `apps/server/src/attachmentStore.ts:263-308`.
pub fn sweep_stale_pending_attachments(attachments_dir: &Path, now: SystemTime) -> usize {
    let Ok(entries) = fs::read_dir(attachments_dir) else {
        return 0;
    };

    let mut deleted = 0usize;
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        if name.contains('/') || name.contains('\\') {
            continue;
        }
        let is_partial = Path::new(name)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("part"));
        if !is_partial {
            let trimmed = match name.strip_suffix(".txt") {
                Some(prefix) if prefix.contains('.') => prefix,
                _ => name,
            };
            if !is_pending_attachment_filename(trimmed) {
                continue;
            }
        }

        let max_age = if is_partial {
            PARTIAL_UPLOAD_MAX_AGE
        } else {
            PENDING_ATTACHMENT_MAX_AGE
        };
        // Never follow symlinks during attachment sweeps (#40528).
        let Ok(metadata) = fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            continue;
        }
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        if let Ok(elapsed) = now.duration_since(modified)
            && elapsed > max_age
            && fs::remove_file(entry.path()).is_ok()
        {
            deleted += 1;
        }
    }

    deleted
}

fn is_pending_attachment_filename(name: &str) -> bool {
    let Some((stem, _ext)) = name.rsplit_once('.') else {
        return false;
    };
    if stem.is_empty() || stem.contains('.') {
        return false;
    }
    let Some(rest) = stem
        .get(..PENDING_ATTACHMENT_THREAD_SEGMENT.len() + 1)
        .filter(|prefix| prefix.eq_ignore_ascii_case("pending-"))
        .and_then(|_| stem.get(PENDING_ATTACHMENT_THREAD_SEGMENT.len() + 1..))
    else {
        return false;
    };
    if rest.len() < 36 {
        return false;
    }
    let Some((uuid_part, suffix)) = rest.split_at_checked(36) else {
        return false;
    };
    if Uuid::parse_str(uuid_part).is_err() {
        return false;
    }
    if suffix.is_empty() {
        return true;
    }
    let Some(ext_suffix) = suffix.strip_prefix('-') else {
        return false;
    };
    (1..=10).contains(&ext_suffix.len()) && ext_suffix.bytes().all(|b| b.is_ascii_alphanumeric())
}

#[cfg(target_os = "linux")]
pub(crate) fn mark_no_copy_on_write_best_effort(directory: &Path) {
    use std::process::Command;

    let Ok(output) = Command::new("stat")
        .args(["-f", "-c", "%T"])
        .arg(directory)
        .output()
    else {
        return;
    };
    if !output.status.success() {
        return;
    }
    if String::from_utf8_lossy(&output.stdout).trim() == "btrfs" {
        let _ = Command::new("chattr").arg("+C").arg(directory).status();
    }
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn mark_no_copy_on_write_best_effort(_directory: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    #[test]
    fn recognizes_valid_pending_attachment_names() {
        let id = Uuid::new_v4();
        assert!(is_pending_attachment_filename(&format!("pending-{id}.png")));
        assert!(is_pending_attachment_filename(&format!(
            "PENDING-{id}-pdf.bin"
        )));
        assert!(!is_pending_attachment_filename(&format!(
            "thread1-{id}.png"
        )));
        assert!(!is_pending_attachment_filename("pending-notauuid.png"));
    }

    #[test]
    fn sweeps_expired_partial_and_retains_fresh_uploads() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = tempfile::tempdir()?;
        let partial = dir.path().join("upload.part");
        File::create(&partial)?;
        let future = SystemTime::now() + Duration::from_secs(3601);
        assert_eq!(sweep_stale_pending_attachments(dir.path(), future), 1);
        assert!(!partial.exists());
        Ok(())
    }
}
