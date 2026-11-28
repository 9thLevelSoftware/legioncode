//! M2 planning validators, argv-only command guards, and deserializers for `workflow_board`.

use serde::{Deserialize, Deserializer, de};

use crate::orchestration::PatchValue;

pub use super::m2_plan_validators::*;
use super::validators::is_valid_artifact_key;

const CREDENTIAL_EXTENSIONS: &[&str] = &[".pem", ".key", ".p12", ".pfx", ".crt", ".der"];
const CREDENTIAL_BASENAMES: &[&str] = &["id_rsa", "id_ed25519", "id_ecdsa", ".npmrc", ".netrc"];
const POSIX_SHELL_BINARIES: &[&str] = &[
    "sh", "bash", "zsh", "dash", "fish", "ksh", "ash", "csh", "tcsh",
];
const SHELL_CONTROL_SNIPPETS: &[&str] = &["&&", "||", "$(", "`", ";", "|", ">", "<"];

/// Returns `true` when `raw` points to a `.env` or credential/key file that must not be ingested.
#[must_use]
pub fn is_credential_file_path(raw: &str) -> bool {
    let normalized = raw.trim().replace('\\', "/").to_ascii_lowercase();
    if normalized.is_empty() {
        return false;
    }
    let segments: Vec<&str> = normalized.split('/').collect();
    if segments.contains(&".ssh") {
        return true;
    }
    let basename = segments.last().copied().unwrap_or("");
    if basename == ".env" || basename.starts_with(".env.") {
        return true;
    }
    if CREDENTIAL_BASENAMES.contains(&basename) {
        return true;
    }
    CREDENTIAL_EXTENSIONS
        .iter()
        .any(|ext| basename.ends_with(ext))
}

/// Returns `true` when `seg` names `.git` metadata (case-insensitive, Windows short-name/dot/stream hardened).
#[must_use]
pub fn is_git_metadata_segment(seg: &str) -> bool {
    let head = seg.split(':').next().unwrap_or("");
    let cleaned = head.trim_end_matches(['.', ' ']).to_ascii_uppercase();
    cleaned == ".GIT" || cleaned == "GIT~1"
}

/// Returns `true` when `raw` is a safe relative workspace path and not a credential file or `.git` metadata path.
#[must_use]
pub fn is_valid_input_path(raw: &str) -> bool {
    let trimmed = raw.trim();
    is_valid_artifact_key(trimmed)
        && !is_credential_file_path(trimmed)
        && !trimmed.split('/').any(is_git_metadata_segment)
}

fn executable_basename(token: &str) -> String {
    let last = token
        .trim()
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    last.strip_suffix(".exe").unwrap_or(&last).to_owned()
}

fn is_posix_inline_flag(arg: &str) -> bool {
    let lower = arg.trim().to_ascii_lowercase();
    if matches!(lower.as_str(), "-c" | "-lc" | "-ic") {
        return true;
    }
    lower.starts_with('-')
        && !lower.starts_with("--")
        && lower.get(1..).is_some_and(|rest| rest.contains('c'))
}

fn is_powershell_inline_flag(arg: &str) -> bool {
    let lower = arg.trim().to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "-c" | "-command" | "-encodedcommand" | "-enc" | "-e"
    )
}

fn is_env_split_string_flag(arg: &str) -> bool {
    let trimmed = arg.trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower == "--split-string" || lower.starts_with("--split-string=") {
        return true;
    }
    trimmed.starts_with('-')
        && !trimmed.starts_with("--")
        && trimmed.get(1..).is_some_and(|rest| rest.contains('S'))
}

fn is_env_two_token_flag(arg: &str) -> bool {
    if matches!(arg, "-u" | "--unset" | "-C" | "--chdir" | "-a" | "--argv0") {
        return true;
    }
    if arg.starts_with('-') && !arg.starts_with("--") && arg.len() > 2 {
        let mut chars = arg[1..].chars();
        if let Some(last) = chars.next_back() {
            return matches!(last, 'u' | 'C' | 'a')
                && chars.all(|ch| matches!(ch, 'i' | '0' | 'v'));
        }
    }
    false
}

fn is_env_single_token_flag(arg: &str) -> bool {
    if matches!(
        arg,
        "-i" | "--ignore-environment" | "-0" | "--null" | "-v" | "--debug"
    ) || arg.starts_with("--unset=")
        || arg.starts_with("--chdir=")
        || arg.starts_with("--argv0=")
    {
        return true;
    }
    if arg.starts_with('-') && !arg.starts_with("--") && arg.len() > 1 {
        let rest = &arg[1..];
        if rest.starts_with(['u', 'C', 'a']) && rest.len() > 1 {
            return true;
        }
        return rest.chars().all(|ch| matches!(ch, 'i' | '0' | 'v'));
    }
    false
}

/// Returns `true` when `argv` is a non-empty argv-only command without shell wrappers or shell control tokens.
#[must_use]
pub fn is_valid_argv_command(argv: &[String]) -> bool {
    if argv.is_empty() {
        return false;
    }
    for token in argv {
        if token.trim().is_empty()
            || token.contains(['\0', '\r', '\n'])
            || SHELL_CONTROL_SNIPPETS
                .iter()
                .any(|snippet| token.contains(snippet))
        {
            return false;
        }
    }

    let mut exe_idx = 0_usize;
    while exe_idx < argv.len() && executable_basename(&argv[exe_idx]) == "env" {
        exe_idx += 1;
        while exe_idx < argv.len() {
            let arg = argv[exe_idx].trim();
            if is_env_split_string_flag(arg) {
                return false;
            }
            if arg == "--" {
                exe_idx += 1;
                break;
            }
            if is_env_two_token_flag(arg) {
                if exe_idx + 1 >= argv.len() {
                    return false;
                }
                exe_idx += 2;
                continue;
            }
            if is_env_single_token_flag(arg) || (!arg.starts_with('-') && arg.contains('=')) {
                exe_idx += 1;
                continue;
            }
            if arg.starts_with('-') {
                return false;
            }
            break;
        }
    }
    if exe_idx >= argv.len() {
        return false;
    }

    let exe = executable_basename(&argv[exe_idx]);
    let rest = &argv[exe_idx + 1..];
    if POSIX_SHELL_BINARIES.contains(&exe.as_str()) && rest.iter().any(|a| is_posix_inline_flag(a))
    {
        return false;
    }
    if exe == "cmd"
        && rest
            .iter()
            .any(|a| matches!(a.trim().to_ascii_lowercase().as_str(), "/c" | "/k"))
    {
        return false;
    }
    if (exe == "powershell" || exe == "pwsh") && rest.iter().any(|a| is_powershell_inline_flag(a)) {
        return false;
    }
    true
}

/// Returns `true` when `raw` is a safe relative artifact key without traversal, root/drive prefixes, or `.git` metadata segments.
#[must_use]
pub fn is_valid_non_git_artifact_key(raw: &str) -> bool {
    let trimmed = raw.trim();
    is_valid_artifact_key(trimmed) && !trimmed.split('/').any(is_git_metadata_segment)
}

pub(crate) fn deserialize_opt_artifact_key_vec<'de, D>(
    deserializer: D,
) -> Result<Option<Vec<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt = Option::<Vec<String>>::deserialize(deserializer)?;
    match opt {
        None => Ok(None),
        Some(items) => {
            let out = items
                .into_iter()
                .map(|raw| {
                    let trimmed = raw.trim();
                    if is_valid_non_git_artifact_key(trimmed) {
                        Ok(trimmed.to_owned())
                    } else {
                        Err(de::Error::custom(
                            "expected a safe relative artifact key without traversal, root/drive prefixes, or .git metadata",
                        ))
                    }
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Some(out))
        }
    }
}

pub(crate) fn deserialize_non_empty_artifact_key_vec<'de, D>(
    deserializer: D,
) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let items = Vec::<String>::deserialize(deserializer)?;
    if items.is_empty() {
        return Err(de::Error::custom("expected at least one relative path"));
    }
    items
        .into_iter()
        .map(|raw| {
            let trimmed = raw.trim();
            if is_valid_non_git_artifact_key(trimmed) {
                Ok(trimmed.to_owned())
            } else {
                Err(de::Error::custom(
                    "expected a safe relative artifact key without traversal, root/drive prefixes, or .git metadata",
                ))
            }
        })
        .collect()
}

pub(crate) fn deserialize_argv_command<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let argv = Vec::<String>::deserialize(deserializer)?;
    let trimmed: Vec<String> = argv.into_iter().map(|s| s.trim().to_owned()).collect();
    if !is_valid_argv_command(&trimmed) {
        return Err(de::Error::custom(
            "expected a valid argv-only command without shell wrappers or control operators",
        ));
    }
    Ok(trimmed)
}

pub(crate) fn deserialize_patch_trimmed_non_empty<'de, D>(
    deserializer: D,
) -> Result<PatchValue<String>, D::Error>
where
    D: Deserializer<'de>,
{
    match PatchValue::<String>::deserialize(deserializer)? {
        PatchValue::Absent => Ok(PatchValue::Absent),
        PatchValue::Null => Ok(PatchValue::Null),
        PatchValue::Set(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Err(de::Error::custom(
                    "string cannot be empty or whitespace-only",
                ));
            }
            Ok(PatchValue::Set(trimmed.to_owned()))
        }
    }
}
