//! Secret name validation, canonical directory confinement (`#39483`), atomic file helpers,
//! and brand-routed home/secret naming helpers.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use legion_contracts::brand::{ACTIVE_BRAND, BrandConfig};
use uuid::Uuid;

use crate::secrets::SecretStoreError;

/// Validates a secret key name before resolving `<secrets_dir>/<name>.bin` (`#39483`).
///
/// Rejects empty names, `.`, `..`, path separators (`/` and `\`), Windows drive/stream
/// colons (`:`), NUL bytes (`\0`), and `..` substrings.
///
/// # Errors
///
/// Returns [`SecretStoreError::InvalidName`] when `name` violates confinement rules.
pub fn validate_secret_name(name: &str) -> Result<(), SecretStoreError> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
        || name.contains(':')
        || name.contains('\0')
        || name.contains("..")
    {
        return Err(SecretStoreError::InvalidName {
            name: name.to_owned(),
        });
    }
    Ok(())
}

/// Validates `name` and verifies `<secrets_dir>/<name>.bin` resolves within canonical `secrets_dir`.
pub(crate) fn resolve_confined_secret_path(
    secrets_dir: &Path,
    name: &str,
) -> Result<PathBuf, SecretStoreError> {
    validate_secret_name(name)?;
    let candidate = secrets_dir.join(format!("{name}.bin"));
    let Ok(canon_dir) = fs::canonicalize(secrets_dir) else {
        return Err(SecretStoreError::InvalidName {
            name: name.to_owned(),
        });
    };
    let Some(parent) = candidate.parent() else {
        return Err(SecretStoreError::InvalidName {
            name: name.to_owned(),
        });
    };
    let Ok(canon_parent) = fs::canonicalize(parent) else {
        return Err(SecretStoreError::InvalidName {
            name: name.to_owned(),
        });
    };
    if canon_parent != canon_dir {
        return Err(SecretStoreError::InvalidName {
            name: name.to_owned(),
        });
    }
    if fs::symlink_metadata(&candidate).is_ok() {
        let Ok(canon_target) = fs::canonicalize(&candidate) else {
            return Err(SecretStoreError::InvalidName {
                name: name.to_owned(),
            });
        };
        if !canon_target.starts_with(&canon_dir) {
            return Err(SecretStoreError::InvalidName {
                name: name.to_owned(),
            });
        }
    }
    Ok(candidate)
}

/// Resolves the `LegionCode` base directory using [`ACTIVE_BRAND`] environment variables and fallback
/// home directory name.
#[must_use]
pub fn resolve_secret_home_from_env<F>(lookup: F, user_home: Option<&Path>) -> Option<PathBuf>
where
    F: FnMut(&str) -> Option<String>,
{
    resolve_secret_home_for_brand(&ACTIVE_BRAND, lookup, user_home)
}

/// Parameterized variant of [`resolve_secret_home_from_env`] for an explicit [`BrandConfig`].
#[must_use]
pub fn resolve_secret_home_for_brand<F>(
    brand: &BrandConfig,
    lookup: F,
    user_home: Option<&Path>,
) -> Option<PathBuf>
where
    F: FnMut(&str) -> Option<String>,
{
    if let Some(raw) = brand.read_env_from("HOME", lookup) {
        return Some(PathBuf::from(raw));
    }
    user_home.map(|home| home.join(brand.home_dir_name))
}

/// Builds a brand-scoped secret key name using [`ACTIVE_BRAND`].
#[must_use]
pub fn branded_secret_name(suffix: &str) -> String {
    branded_secret_name_for_brand(&ACTIVE_BRAND, suffix)
}

/// Builds a brand-scoped secret key name for an explicit [`BrandConfig`].
#[must_use]
pub fn branded_secret_name_for_brand(brand: &BrandConfig, suffix: &str) -> String {
    let trimmed = suffix.trim_start_matches('-');
    format!("{}-{trimmed}", brand.url_scheme)
}

pub(crate) fn temp_path_for(secret_path: &Path) -> PathBuf {
    let mut os = secret_path.as_os_str().to_os_string();
    os.push(format!(".{}.tmp", Uuid::new_v4()));
    PathBuf::from(os)
}

pub(crate) fn write_temp_secret_file(temp_path: &Path, value: &[u8]) -> Result<(), std::io::Error> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(temp_path)?;
    file.write_all(value)?;
    file.sync_all()?;
    drop(file);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(temp_path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

pub(crate) fn random_bytes(len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        let block = *Uuid::new_v4().as_bytes();
        let remaining = len - out.len();
        let take = remaining.min(block.len());
        out.extend_from_slice(&block[..take]);
    }
    out
}
