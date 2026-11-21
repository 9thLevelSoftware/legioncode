//! Built-in `OpenCode`, Pi, Muse, and ACP Registry driver helpers.

use std::collections::BTreeMap;

use serde_json::{Map, Value};
use t3_protocol::provider_env::SanitizedEnv;

use super::{ProviderLaunchSpec, ProviderProbeSpec, build_child_env};

/// Minimum supported `OpenCode` server version.
pub const MINIMUM_OPENCODE_VERSION: &str = "1.14.19";
/// Minimum supported Pi CLI version.
pub const MINIMUM_PI_VERSION: &str = "0.84.3";
/// Supported Muse Server Protocol (MSP) schema version.
pub const MUSE_PROTOCOL_VERSION: u32 = 1;

/// Errors raised when validating Pi CLI arguments before spawn.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PiLaunchError {
    /// Caller attempted to pass a flag controlled exclusively by T3 Code (`--mode`, `-p`, `--print`).
    #[error("reserved Pi flag cannot be overridden in extra args: {flag}")]
    ReservedFlag {
        /// The disallowed flag supplied by the caller.
        flag: String,
    },
    /// `--provider` was supplied without a pairing `--model`.
    #[error("Pi requires `--model` whenever `--provider` is specified")]
    ProviderWithoutModel,
}

/// Errors raised when validating ACP Registry local executables or archive paths.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegistryExecError {
    /// Executable path is empty or contains invalid characters.
    #[error("invalid ACP registry executable path: {reason}")]
    InvalidPath {
        /// Reason the path was rejected.
        reason: &'static str,
    },
    /// Windows `.cmd` / `.bat` batch shims are rejected to prevent `cmd.exe` shell execution.
    #[error("Windows `.cmd`/`.bat` shims are disallowed for ACP registry command: {path}")]
    WindowsBatchShimRejected {
        /// Rejected shim path.
        path: String,
    },
}

/// Builds `OpenCode` child environment variables for 1.x vs 2.x authentication (`BUG-12`).
///
/// - `OpenCode` 1.x uses `OPENCODE_SERVER_PASSWORD`.
/// - `OpenCode` 2.x uses `OPENCODE_PASSWORD` and explicitly strips inherited `OPENCODE_SERVER_PASSWORD`.
#[must_use]
pub fn opencode_server_env(
    env: &SanitizedEnv,
    major_version: u32,
    password: &str,
) -> BTreeMap<String, String> {
    let mut overrides = BTreeMap::new();
    let removed: &[&str] = if major_version >= 2 {
        overrides.insert("OPENCODE_PASSWORD".to_owned(), password.to_owned());
        &["OPENCODE_SERVER_PASSWORD"]
    } else {
        overrides.insert("OPENCODE_SERVER_PASSWORD".to_owned(), password.to_owned());
        overrides.insert("OPENCODE_PASSWORD".to_owned(), password.to_owned());
        &[]
    };
    build_child_env(env, removed, &overrides)
}

/// Computes the HTTP `Authorization: Basic ...` header value for `OpenCode` (`opencode:<password>`).
#[must_use]
pub fn opencode_basic_auth_header(password: &str) -> String {
    let creds = format!("opencode:{password}");
    format!("Basic {}", encode_base64(creds.as_bytes()))
}

/// Backfills missing `recent` strings on `OpenCode` compaction entries returned by
/// `/api/session/<id>/message`, reading `metadata.recent` when present or `""`.
///
/// Returns the number of entries patched.
pub fn fill_compaction_recents(payload: &mut Value) -> usize {
    match payload {
        Value::Array(items) => patch_compaction_slice(items),
        Value::Object(obj) => {
            let mut count = 0usize;
            for key in ["items", "messages", "data"] {
                if let Some(Value::Array(items)) = obj.get_mut(key) {
                    count += patch_compaction_slice(items);
                }
            }
            if count == 0 && patch_single_compaction(obj) {
                count += 1;
            }
            count
        }
        _ => 0,
    }
}

fn patch_compaction_slice(items: &mut [Value]) -> usize {
    let mut patched = 0usize;
    for item in items {
        if let Some(obj) = item.as_object_mut()
            && patch_single_compaction(obj)
        {
            patched += 1;
        }
    }
    patched
}

fn patch_single_compaction(obj: &mut Map<String, Value>) -> bool {
    let top_compaction = obj.get("type").and_then(Value::as_str) == Some("compaction");
    let info_compaction = obj
        .get("info")
        .and_then(|v| v.get("type"))
        .and_then(Value::as_str)
        == Some("compaction");
    if !top_compaction && !info_compaction {
        return false;
    }

    if let Some(info) = obj.get_mut("info").and_then(Value::as_object_mut)
        && info.get("type").and_then(Value::as_str) == Some("compaction")
    {
        return ensure_recent_field(info);
    }
    ensure_recent_field(obj)
}

fn ensure_recent_field(target: &mut Map<String, Value>) -> bool {
    if target.get("recent").is_some_and(Value::is_string) {
        return false;
    }
    let fallback = target
        .get("metadata")
        .and_then(|m| m.get("recent"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    target.insert("recent".to_owned(), Value::String(fallback));
    true
}

/// Validates and builds Pi CLI RPC launch arguments (`--mode rpc`, optional `--no-session`).
pub fn pi_launch_args(
    ephemeral: bool,
    model: Option<&str>,
    provider: Option<&str>,
    extra_args: &[String],
) -> Result<Vec<String>, PiLaunchError> {
    let model = model.map(str::trim).filter(|s| !s.is_empty());
    let provider = provider.map(str::trim).filter(|s| !s.is_empty());
    if provider.is_some() && model.is_none() {
        return Err(PiLaunchError::ProviderWithoutModel);
    }

    for arg in extra_args {
        let token = arg.split('=').next().unwrap_or(arg).trim();
        if matches!(token, "--mode" | "--print" | "-p" | "--no-session") {
            return Err(PiLaunchError::ReservedFlag { flag: arg.clone() });
        }
    }

    let mut args = vec!["--mode".to_owned(), "rpc".to_owned()];
    if ephemeral {
        args.push("--no-session".to_owned());
    }
    if let Some(prov) = provider {
        args.push("--provider".to_owned());
        args.push(prov.to_owned());
    }
    if let Some(m) = model {
        args.push("--model".to_owned());
        args.push(m.to_owned());
    }
    args.extend(extra_args.iter().cloned());
    Ok(args)
}

/// Builds a Pi offline probe specification (`--offline` + `MISE_OFFLINE=1`, `BUG-12`).
#[must_use]
pub fn pi_probe_spec(env: &SanitizedEnv, binary_path: &str) -> ProviderProbeSpec {
    let mut overrides = BTreeMap::new();
    overrides.insert("MISE_OFFLINE".to_owned(), "1".to_owned());
    ProviderProbeSpec {
        executable: binary_path.to_owned(),
        args: vec!["--offline".to_owned(), "--version".to_owned()],
        env: build_child_env(env, &[], &overrides),
        removed_env_keys: Vec::new(),
    }
}

/// Strips Win32 verbatim `\\?\` / `\\?\UNC\` prefixes so Muse MSP v1 sees canonical paths.
#[must_use]
pub fn muse_verbatim_path(path: &str, is_windows: bool) -> String {
    if !is_windows {
        return path.to_owned();
    }
    if let Some(unc_rest) = strip_prefix_ci(path, r"\\?\UNC\") {
        return format!(r"\\{unc_rest}");
    }
    if let Some(local_rest) = path.strip_prefix(r"\\?\") {
        return local_rest.to_owned();
    }
    path.to_owned()
}

/// Builds a sanitized Muse MSP v1 launch specification (`META_API_KEY` stripped,
/// `MUSE_NO_AUTO_UPDATE=1`).
#[must_use]
pub fn muse_launch_spec(
    env: &SanitizedEnv,
    binary_path: &str,
    cwd: Option<&str>,
) -> ProviderLaunchSpec {
    let mut overrides = BTreeMap::new();
    overrides.insert("MUSE_NO_AUTO_UPDATE".to_owned(), "1".to_owned());
    let removed = ["META_API_KEY"];
    ProviderLaunchSpec {
        executable: binary_path.to_owned(),
        args: vec![
            "serve".to_owned(),
            "--msp-version".to_owned(),
            MUSE_PROTOCOL_VERSION.to_string(),
        ],
        env: build_child_env(env, &removed, &overrides),
        removed_env_keys: removed.iter().map(|k| (*k).to_owned()).collect(),
        cwd: cwd.map(|p| muse_verbatim_path(p, env.is_windows())),
        cancel_behavior: "immediate",
    }
}

/// Validates a local ACP Registry executable path, rejecting Win32 `.cmd`/`.bat` shims.
pub fn validate_registry_executable(
    path: &str,
    is_windows: bool,
) -> Result<String, RegistryExecError> {
    let normalized = normalize_registry_command_path(path)?;
    if is_windows
        && std::path::Path::new(&normalized)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("cmd") || ext.eq_ignore_ascii_case("bat"))
    {
        return Err(RegistryExecError::WindowsBatchShimRejected { path: normalized });
    }
    Ok(normalized)
}

/// Normalizes an ACP Registry binary/archive command path while blocking `..` traversal.
pub fn normalize_registry_command_path(raw: &str) -> Result<String, RegistryExecError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(RegistryExecError::InvalidPath {
            reason: "empty command path",
        });
    }
    if trimmed.contains('\0') {
        return Err(RegistryExecError::InvalidPath {
            reason: "path contains NUL byte",
        });
    }
    for segment in trimmed.split(['/', '\\']) {
        if segment == ".." {
            return Err(RegistryExecError::InvalidPath {
                reason: "parent directory traversal is prohibited",
            });
        }
    }
    Ok(trimmed.to_owned())
}

fn strip_prefix_ci<'a>(input: &'a str, prefix: &str) -> Option<&'a str> {
    let head = input.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| input.get(prefix.len()..).unwrap_or(""))
}

fn encode_base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = chunk.get(1).copied().map_or(0, u32::from);
        let b2 = chunk.get(2).copied().map_or(0, u32::from);
        let triple = (b0 << 16) | (b1 << 8) | b2;

        out.push(char::from(TABLE[((triple >> 18) & 0x3F) as usize]));
        out.push(char::from(TABLE[((triple >> 12) & 0x3F) as usize]));
        if chunk.len() > 1 {
            out.push(char::from(TABLE[((triple >> 6) & 0x3F) as usize]));
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(char::from(TABLE[(triple & 0x3F) as usize]));
        } else {
            out.push('=');
        }
    }
    out
}
