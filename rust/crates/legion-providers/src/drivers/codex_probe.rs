//! Codex local binary `(path, mtime, size, ...)` probe cache (`9168ca73d0`).

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use crate::core::compatibility::Semver;

/// Whether a local Codex binary is a native release or a package/shim launcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CodexLocalBinaryKind {
    /// Native Codex binary accompanied by `codex-code-mode-host`.
    Native,
    /// Package manager shim, script shebang, or shim manager binary (`mise`, `volta-shim`, `aqua-proxy`).
    Launcher,
}

/// Binary metadata fingerprint `(path, real_path, mtime, size, ino, npm_version)` used to cache `--version`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexBinaryFingerprint {
    /// Configured or PATH-resolved executable path.
    pub executable_path: String,
    /// Canonicalized target path (follows symlinks).
    pub real_executable_path: String,
    /// File byte length.
    pub size: u64,
    /// Last modification timestamp in milliseconds since Unix epoch.
    pub mtime_unix_ms: u128,
    /// Unix inode (0 on non-Unix platforms).
    pub ino: u64,
    /// Resolved `@openai/codex` package version when installed via npm.
    pub npm_version: Option<String>,
    /// Native binary vs launcher classification.
    pub kind: CodexLocalBinaryKind,
}

impl CodexBinaryFingerprint {
    /// Deterministic string key combining path, size, mtime, inode, and npm package version.
    #[must_use]
    pub fn key(&self) -> String {
        format!(
            "{}:{}:{}:{}:{}",
            self.real_executable_path,
            self.size,
            self.mtime_unix_ms,
            self.ino,
            self.npm_version.as_deref().unwrap_or("")
        )
    }

    /// Only native binaries and npm launchers with a readable `package.json` version persist across restarts.
    #[must_use]
    pub fn should_persist(&self) -> bool {
        self.kind == CodexLocalBinaryKind::Native || self.npm_version.is_some()
    }
}

/// On-disk cached Codex local probe record (`caches/codex-local-probe.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexLocalProbeRecord {
    /// Original executable path that was probed.
    pub executable_path: String,
    /// Fingerprint key of the probed binary.
    pub fingerprint: String,
    /// Detected `codex-cli X.Y.Z` semantic version.
    pub version: String,
}

type ProbeMemoryMap = HashMap<PathBuf, (String, Option<String>)>;

/// Thread-safe in-memory + optional file-backed cache mapping `(path, mtime, size, ...)` to version.
#[derive(Debug, Clone, Default)]
pub struct CodexLocalProbeCache {
    memory: Arc<Mutex<ProbeMemoryMap>>,
    persist_path: Option<PathBuf>,
}

impl CodexLocalProbeCache {
    /// Creates an in-memory cache without disk persistence.
    #[must_use]
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// Creates a cache backed by a JSON file (such as `<baseDir>/caches/codex-local-probe.json`).
    #[must_use]
    pub fn with_persist_path(persist_path: impl Into<PathBuf>) -> Self {
        Self {
            memory: Arc::new(Mutex::new(HashMap::new())),
            persist_path: Some(persist_path.into()),
        }
    }

    /// Resolves the Codex version for `executable_path`, running `probe` only on cache miss or binary change.
    pub fn resolve_version_with<F, E>(
        &self,
        executable_path: &Path,
        is_windows: bool,
        probe: F,
    ) -> Result<Option<String>, E>
    where
        F: FnOnce(&Path) -> Result<Option<String>, E>,
    {
        let Some(fp) = inspect_codex_binary_fingerprint(executable_path, is_windows) else {
            return Ok(None);
        };
        let key = fp.key();
        {
            let guard = self.memory.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some((cached_fp, cached_ver)) = guard.get(executable_path)
                && cached_fp == &key
            {
                return Ok(cached_ver.clone());
            }
        }
        if let Some(persist_path) = &self.persist_path
            && let Some(ver) = load_matching_persisted_probe(persist_path, executable_path, &key)
        {
            let mut guard = self.memory.lock().unwrap_or_else(PoisonError::into_inner);
            guard.insert(executable_path.to_path_buf(), (key, Some(ver.clone())));
            return Ok(Some(ver));
        }
        let probed = probe(executable_path)?;
        let parsed = probed.as_deref().and_then(parse_codex_cli_version);
        if let Some(version) = &parsed
            && fp.should_persist()
            && let Some(persist_path) = &self.persist_path
        {
            let _ = save_persisted_probe(persist_path, executable_path, &key, version);
        }
        let mut guard = self.memory.lock().unwrap_or_else(PoisonError::into_inner);
        guard.insert(executable_path.to_path_buf(), (key, parsed.clone()));
        Ok(parsed)
    }
}

fn load_matching_persisted_probe(
    persist_path: &Path,
    executable_path: &Path,
    expected_fp: &str,
) -> Option<String> {
    let raw = fs::read_to_string(persist_path).ok()?;
    let record: CodexLocalProbeRecord = serde_json::from_str(&raw).ok()?;
    let exec_str = executable_path.to_string_lossy();
    if record.executable_path == exec_str.as_ref()
        && record.fingerprint == expected_fp
        && Semver::parse_strict(&record.version).is_some()
    {
        Some(record.version)
    } else {
        None
    }
}

fn save_persisted_probe(
    persist_path: &Path,
    executable_path: &Path,
    fingerprint: &str,
    version: &str,
) -> std::io::Result<()> {
    if let Some(parent) = persist_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let record = CodexLocalProbeRecord {
        executable_path: executable_path.to_string_lossy().into_owned(),
        fingerprint: fingerprint.to_owned(),
        version: version.to_owned(),
    };
    let encoded = serde_json::to_string(&record).map_err(std::io::Error::other)?;
    fs::write(persist_path, encoded)
}

/// Parses `codex-cli X.Y.Z` (or bare `X.Y.Z`) into a strict semantic version string.
#[must_use]
pub fn parse_codex_cli_version(output: &str) -> Option<String> {
    let trimmed = output.trim();
    let candidate = trimmed.strip_prefix("codex-cli ").unwrap_or(trimmed);
    Semver::parse_strict(candidate).map(|v| v.to_string())
}

/// Inspects a local Codex binary path on disk and builds its cache fingerprint.
#[must_use]
pub fn inspect_codex_binary_fingerprint(
    executable_path: &Path,
    is_windows: bool,
) -> Option<CodexBinaryFingerprint> {
    let real_path =
        fs::canonicalize(executable_path).unwrap_or_else(|_| executable_path.to_path_buf());
    let kind = classify_local_binary(&real_path, is_windows)?;
    let npm_version = match kind {
        CodexLocalBinaryKind::Launcher => read_npm_codex_version(executable_path, &real_path),
        CodexLocalBinaryKind::Native => None,
    };
    let meta = fs::metadata(executable_path).ok()?;
    let mtime_unix_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis());
    Some(CodexBinaryFingerprint {
        executable_path: executable_path.to_string_lossy().into_owned(),
        real_executable_path: real_path.to_string_lossy().into_owned(),
        size: meta.len(),
        mtime_unix_ms,
        ino: unix_ino(&meta),
        npm_version,
        kind,
    })
}

#[cfg(unix)]
fn unix_ino(meta: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    meta.ino()
}

#[cfg(not(unix))]
fn unix_ino(_meta: &fs::Metadata) -> u64 {
    0
}

fn classify_local_binary(real_path: &Path, is_windows: bool) -> Option<CodexLocalBinaryKind> {
    let file_name = real_path.file_name()?.to_string_lossy();
    let stem = file_name
        .strip_suffix(".exe")
        .or_else(|| file_name.strip_suffix(".EXE"))
        .unwrap_or(&file_name);
    if stem.eq_ignore_ascii_case("volta-shim")
        || stem.eq_ignore_ascii_case("mise")
        || stem.eq_ignore_ascii_case("aqua-proxy")
    {
        return Some(CodexLocalBinaryKind::Launcher);
    }
    if is_windows {
        let is_exe = real_path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("exe"));
        if !is_exe {
            return Some(CodexLocalBinaryKind::Launcher);
        }
        let path_str = real_path.to_string_lossy();
        if let Some(base) = path_str.get(..path_str.len().saturating_sub(4)) {
            for ext in [".bunx", ".shim"] {
                let marker = PathBuf::from(format!("{base}{ext}"));
                if fs::metadata(marker).is_ok_and(|m| m.is_file() && m.len() > 0) {
                    return Some(CodexLocalBinaryKind::Launcher);
                }
            }
        }
    } else if has_shebang_header(real_path) {
        return Some(CodexLocalBinaryKind::Launcher);
    }

    let host_name = if is_windows {
        "codex-code-mode-host.exe"
    } else {
        "codex-code-mode-host"
    };
    let host_path = real_path.parent()?.join(host_name);
    let host_meta = fs::metadata(host_path).ok()?;
    if host_meta.is_file() && host_meta.len() > 0 && is_executable(&host_meta, is_windows) {
        Some(CodexLocalBinaryKind::Native)
    } else {
        None
    }
}

fn has_shebang_header(path: &Path) -> bool {
    let Ok(mut file) = fs::File::open(path) else {
        return false;
    };
    let mut header = [0_u8; 2];
    file.read_exact(&mut header).is_ok() && header == *b"#!"
}

#[cfg(unix)]
fn is_executable(meta: &fs::Metadata, is_windows: bool) -> bool {
    use std::os::unix::fs::PermissionsExt;
    is_windows || (meta.permissions().mode() & 0o111) != 0
}

#[cfg(not(unix))]
fn is_executable(_meta: &fs::Metadata, _is_windows: bool) -> bool {
    true
}

fn read_npm_codex_version(executable_path: &Path, real_path: &Path) -> Option<String> {
    #[derive(Deserialize)]
    struct NpmPackage {
        name: String,
        version: String,
    }
    let candidates = [
        executable_path
            .parent()
            .map(|p| p.join("node_modules/@openai/codex/package.json")),
        real_path
            .parent()
            .and_then(Path::parent)
            .map(|p| p.join("package.json")),
    ];
    for candidate in candidates.into_iter().flatten() {
        let Ok(raw) = fs::read_to_string(candidate) else {
            continue;
        };
        if let Ok(pkg) = serde_json::from_str::<NpmPackage>(&raw)
            && pkg.name == "@openai/codex"
            && !pkg.version.trim().is_empty()
        {
            return Some(pkg.version);
        }
    }
    None
}
