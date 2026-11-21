//! File-backed server secret storage (`0o600` `.bin` files) and `op://` secret references.
//!
//! Mirrors `apps/server/src/auth/ServerSecretStore.ts` (`BUG-16`: zero dead OS keyring
//! dependency), `packages/secret-provider-onepassword/src/shared/definition.ts`, and
//! `packages/secret-provider-core/src/server/SecretResolver.ts` (`BUG-15`: TTL cache via
//! [`t3_protocol::auth::TtlSecretCache`] that never memoizes failed, `None`, or blank reads).

#[path = "secrets_ref.rs"]
mod ref_cache;

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use thiserror::Error;
use uuid::Uuid;

pub use ref_cache::{
    CachedSecretResolver, MAX_SECRET_PROVIDER_ID_LENGTH, MAX_SECRET_REF_LENGTH, SecretRef,
    SecretRefError, SecretReference,
};

/// Errors produced by [`FileSecretStore`].
#[derive(Debug, Error)]
pub enum SecretStoreError {
    /// Secret key name is empty or contains path traversal characters.
    #[error("invalid secret name '{name}': must be non-empty without path separators or '..'")]
    InvalidName {
        /// Rejected secret name.
        name: String,
    },
    /// Random secret byte length must be greater than zero.
    #[error("random secret length must be positive, got {bytes_len}")]
    InvalidLength {
        /// Requested byte count.
        bytes_len: usize,
    },
    /// Failed to create or secure the `<state_dir>/secrets` directory.
    #[error("failed to prepare secrets directory '{}': {source}", path.display())]
    Directory {
        /// Target directory path.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// Failed to read an existing secret file.
    #[error("failed to read {resource} at '{}': {source}", path.display())]
    Read {
        /// Human-readable resource identifier.
        resource: String,
        /// Secret `.bin` path.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// Failed to write, sync, secure, or atomically rename a secret file.
    #[error("failed to persist {resource} at '{}': {source}", path.display())]
    Persist {
        /// Human-readable resource identifier.
        resource: String,
        /// Secret `.bin` path.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// Exclusive creation failed because the secret already exists.
    #[error("{resource} already exists at '{}'", path.display())]
    AlreadyExists {
        /// Human-readable resource identifier.
        resource: String,
        /// Secret `.bin` path.
        path: PathBuf,
    },
    /// Failed to remove a secret file.
    #[error("failed to remove {resource} at '{}': {source}", path.display())]
    Remove {
        /// Human-readable resource identifier.
        resource: String,
        /// Secret `.bin` path.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// Stored secret bytes were not valid UTF-8.
    #[error("failed to decode {resource} as UTF-8: {source}")]
    InvalidUtf8 {
        /// Human-readable resource identifier.
        resource: String,
        /// Underlying UTF-8 error.
        #[source]
        source: std::string::FromUtf8Error,
    },
}

/// File-backed binary secret store persisting `<secrets_dir>/<name>.bin` with atomic writes.
///
/// Matches `apps/server/src/auth/ServerSecretStore.ts`: directories are secured to `0o700` and
/// `.bin` files to `0o600` on Unix, with temp-file write + `sync_all` + atomic rename (`BUG-16`).
///
/// # Examples
///
/// ```
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use t3_remote::secrets::FileSecretStore;
///
/// let dir = tempfile::tempdir()?;
/// let store = FileSecretStore::from_state_dir(dir.path())?;
/// store.set("peer-token", b"top-secret")?;
/// assert_eq!(store.get("peer-token")?.as_deref(), Some(b"top-secret".as_slice()));
/// assert!(store.remove("peer-token")?);
/// assert_eq!(store.get("peer-token")?, None);
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSecretStore {
    secrets_dir: PathBuf,
}

impl FileSecretStore {
    /// Opens or initializes `<state_dir>/secrets`.
    pub fn from_state_dir(state_dir: impl AsRef<Path>) -> Result<Self, SecretStoreError> {
        Self::new(state_dir.as_ref().join("secrets"))
    }

    /// Opens or initializes an explicit `secrets_dir` (`0o700` on Unix).
    pub fn new(secrets_dir: impl AsRef<Path>) -> Result<Self, SecretStoreError> {
        let dir = secrets_dir.as_ref().to_path_buf();
        fs::create_dir_all(&dir).map_err(|source| SecretStoreError::Directory {
            path: dir.clone(),
            source,
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).map_err(|source| {
                SecretStoreError::Directory {
                    path: dir.clone(),
                    source,
                }
            })?;
        }
        Ok(Self { secrets_dir: dir })
    }

    /// Returns the directory holding `.bin` secret files.
    pub fn directory(&self) -> &Path {
        &self.secrets_dir
    }

    /// Resolves `<secrets_dir>/<name>.bin` after validating `name`.
    pub fn secret_path(&self, name: &str) -> Result<PathBuf, SecretStoreError> {
        validate_secret_name(name)?;
        Ok(self.secrets_dir.join(format!("{name}.bin")))
    }

    /// Reads raw secret bytes for `name`, returning `Ok(None)` when absent.
    pub fn get(&self, name: &str) -> Result<Option<Vec<u8>>, SecretStoreError> {
        let path = self.secret_path(name)?;
        match fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(SecretStoreError::Read {
                resource: format!("secret {name}"),
                path,
                source,
            }),
        }
    }

    /// Reads a UTF-8 secret string for `name`.
    pub fn get_utf8(&self, name: &str) -> Result<Option<String>, SecretStoreError> {
        let Some(bytes) = self.get(name)? else {
            return Ok(None);
        };
        String::from_utf8(bytes)
            .map(Some)
            .map_err(|source| SecretStoreError::InvalidUtf8 {
                resource: format!("secret {name}"),
                source,
            })
    }

    /// Atomically writes `value` to `<name>.bin` via temp-file + `sync_all` + rename (`0o600`).
    pub fn set(&self, name: &str, value: &[u8]) -> Result<(), SecretStoreError> {
        let secret_path = self.secret_path(name)?;
        let temp_path = temp_path_for(&secret_path);
        let write_result = (|| -> Result<(), std::io::Error> {
            write_temp_secret_file(&temp_path, value)?;
            fs::rename(&temp_path, &secret_path)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&secret_path, fs::Permissions::from_mode(0o600))?;
            }
            Ok(())
        })();
        if let Err(source) = write_result {
            let _ = fs::remove_file(&temp_path);
            return Err(SecretStoreError::Persist {
                resource: format!("secret {name}"),
                path: secret_path,
                source,
            });
        }
        Ok(())
    }

    /// Atomically stores UTF-8 `value` under `name`.
    pub fn set_utf8(&self, name: &str, value: &str) -> Result<(), SecretStoreError> {
        self.set(name, value.as_bytes())
    }

    /// Exclusively creates `<name>.bin` without overwriting an existing secret (`hard_link`).
    pub fn create(&self, name: &str, value: &[u8]) -> Result<(), SecretStoreError> {
        let secret_path = self.secret_path(name)?;
        let temp_path = temp_path_for(&secret_path);
        let result = (|| -> Result<(), SecretStoreError> {
            write_temp_secret_file(&temp_path, value).map_err(|source| {
                SecretStoreError::Persist {
                    resource: format!("secret {name}"),
                    path: secret_path.clone(),
                    source,
                }
            })?;
            match fs::hard_link(&temp_path, &secret_path) {
                Ok(()) => Ok(()),
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                    Err(SecretStoreError::AlreadyExists {
                        resource: format!("secret {name}"),
                        path: secret_path.clone(),
                    })
                }
                Err(source) => Err(SecretStoreError::Persist {
                    resource: format!("secret {name}"),
                    path: secret_path.clone(),
                    source,
                }),
            }
        })();
        let _ = fs::remove_file(&temp_path);
        result
    }

    /// Returns an existing non-empty secret or generates and creates `bytes_len` CSPRNG bytes.
    ///
    /// Mirrors `ServerSecretStore.getOrCreateRandom`: zero-byte leftover files are removed and
    /// regenerated, and concurrent `AlreadyExists` races re-read the winning secret.
    pub fn get_or_create_random(
        &self,
        name: &str,
        bytes_len: usize,
    ) -> Result<Vec<u8>, SecretStoreError> {
        if bytes_len == 0 {
            return Err(SecretStoreError::InvalidLength { bytes_len });
        }
        if let Some(existing) = self.get(name)? {
            if !existing.is_empty() {
                return Ok(existing);
            }
            let _ = self.remove(name)?;
        }

        let generated = random_bytes(bytes_len);
        match self.create(name, &generated) {
            Ok(()) => Ok(generated),
            Err(SecretStoreError::AlreadyExists { .. }) => {
                self.get(name)?.ok_or_else(|| SecretStoreError::Read {
                    resource: format!("secret {name}"),
                    path: self.secrets_dir.join(format!("{name}.bin")),
                    source: std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "secret disappeared after concurrent creation",
                    ),
                })
            }
            Err(err) => Err(err),
        }
    }

    /// Deletes `<name>.bin`, returning `Ok(true)` if removed or `Ok(false)` when absent.
    pub fn remove(&self, name: &str) -> Result<bool, SecretStoreError> {
        let path = self.secret_path(name)?;
        match fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(source) => Err(SecretStoreError::Remove {
                resource: format!("secret {name}"),
                path,
                source,
            }),
        }
    }
}

fn validate_secret_name(name: &str) -> Result<(), SecretStoreError> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
        || name.contains('\0')
        || name.contains("..")
    {
        return Err(SecretStoreError::InvalidName {
            name: name.to_owned(),
        });
    }
    Ok(())
}

fn temp_path_for(secret_path: &Path) -> PathBuf {
    let mut os = secret_path.as_os_str().to_os_string();
    os.push(format!(".{}.tmp", Uuid::new_v4()));
    PathBuf::from(os)
}

fn write_temp_secret_file(temp_path: &Path, value: &[u8]) -> Result<(), std::io::Error> {
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

fn random_bytes(len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        let block = *Uuid::new_v4().as_bytes();
        let remaining = len - out.len();
        let take = remaining.min(block.len());
        out.extend_from_slice(&block[..take]);
    }
    out
}
