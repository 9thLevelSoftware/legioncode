//! Deserialization and input validators for Project Workflow Board contracts.

use serde::{Deserialize, Deserializer, de};

use super::m2_validators::{
    deserialize_argv_command, is_git_metadata_segment, is_valid_argv_command, is_valid_input_path,
    is_valid_non_git_artifact_key,
};
use crate::orchestration::PatchValue;

const ARTIFACT_KEY_MAX_CHARS: usize = 256;

/// Returns `true` when `raw` trims to a safe relative artifact key without traversal or drive/root prefixes.
#[must_use]
pub fn is_valid_artifact_key(raw: &str) -> bool {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.chars().count() > ARTIFACT_KEY_MAX_CHARS {
        return false;
    }
    if trimmed.contains('\0')
        || trimmed.contains('\\')
        || trimmed.contains(':')
        || trimmed.starts_with('/')
    {
        return false;
    }
    trimmed
        .split('/')
        .all(|seg| !seg.is_empty() && seg != "." && seg != "..")
}

pub(crate) fn deserialize_trimmed_non_empty<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = String::deserialize(deserializer)?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(de::Error::custom(
            "string cannot be empty or whitespace-only",
        ));
    }
    Ok(trimmed.to_owned())
}

pub(crate) fn deserialize_opt_trimmed_non_empty<'de, D>(
    deserializer: D,
) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt = Option::<String>::deserialize(deserializer)?;
    match opt {
        None => Ok(None),
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Err(de::Error::custom(
                    "string cannot be empty or whitespace-only",
                ));
            }
            Ok(Some(trimmed.to_owned()))
        }
    }
}

pub(crate) fn deserialize_trimmed_non_empty_vec<'de, D>(
    deserializer: D,
) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let items = Vec::<String>::deserialize(deserializer)?;
    items
        .into_iter()
        .map(|raw| {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                Err(de::Error::custom(
                    "string cannot be empty or whitespace-only",
                ))
            } else {
                Ok(trimmed.to_owned())
            }
        })
        .collect()
}

pub(crate) fn deserialize_opt_trimmed_non_empty_vec<'de, D>(
    deserializer: D,
) -> Result<Option<Vec<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt = Option::<Vec<String>>::deserialize(deserializer)?;
    match opt {
        None => Ok(None),
        Some(items) => {
            let trimmed = items
                .into_iter()
                .map(|raw| {
                    let t = raw.trim();
                    if t.is_empty() {
                        Err(de::Error::custom(
                            "string cannot be empty or whitespace-only",
                        ))
                    } else {
                        Ok(t.to_owned())
                    }
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Some(trimmed))
        }
    }
}

pub(crate) fn deserialize_artifact_key<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = String::deserialize(deserializer)?;
    let trimmed = raw.trim();
    if !is_valid_artifact_key(trimmed) {
        return Err(de::Error::custom(
            "expected a safe relative artifact key without traversal or root/drive prefixes",
        ));
    }
    Ok(trimmed.to_owned())
}

pub(crate) fn deserialize_opt_artifact_key<'de, D>(
    deserializer: D,
) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt = Option::<String>::deserialize(deserializer)?;
    match opt {
        None => Ok(None),
        Some(raw) => {
            let trimmed = raw.trim();
            if !is_valid_artifact_key(trimmed) {
                return Err(de::Error::custom(
                    "expected a safe relative artifact key without traversal or root/drive prefixes",
                ));
            }
            Ok(Some(trimmed.to_owned()))
        }
    }
}

pub(crate) fn deserialize_patch_artifact_key<'de, D>(
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
            if !is_valid_artifact_key(trimmed) || trimmed.split('/').any(is_git_metadata_segment) {
                return Err(de::Error::custom(
                    "expected a safe relative artifact key without traversal, root/drive prefixes, or .git metadata",
                ));
            }
            Ok(PatchValue::Set(trimmed.to_owned()))
        }
    }
}

pub(crate) fn deserialize_artifact_key_vec<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let items = Vec::<String>::deserialize(deserializer)?;
    items
        .into_iter()
        .map(|raw| {
            let trimmed = raw.trim();
            if is_valid_artifact_key(trimmed) {
                Ok(trimmed.to_owned())
            } else {
                Err(de::Error::custom(
                    "expected a safe relative artifact key without traversal or root/drive prefixes",
                ))
            }
        })
        .collect()
}

pub(crate) fn deserialize_positive_revision<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    let value = u64::deserialize(deserializer)?;
    if value == 0 {
        return Err(de::Error::custom("expected positive revision >= 1"));
    }
    Ok(value)
}

pub(crate) fn deserialize_non_empty_trimmed_vec<'de, D>(
    deserializer: D,
) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let items = deserialize_trimmed_non_empty_vec(deserializer)?;
    if items.is_empty() {
        return Err(de::Error::custom("expected a non-empty array"));
    }
    Ok(items)
}

pub(crate) fn deserialize_present_some<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

pub(crate) fn deserialize_present_trimmed_non_empty<'de, D>(
    deserializer: D,
) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_trimmed_non_empty(deserializer).map(Some)
}

pub(crate) fn deserialize_present_positive_revision<'de, D>(
    deserializer: D,
) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_positive_revision(deserializer).map(Some)
}

pub(crate) fn deserialize_opt_positive_revision<'de, D>(
    deserializer: D,
) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt = Option::<u64>::deserialize(deserializer)?;
    match opt {
        None => Ok(None),
        Some(0) => Err(de::Error::custom("expected positive revision >= 1")),
        Some(v) => Ok(Some(v)),
    }
}

pub(crate) fn deserialize_present_trimmed_non_empty_vec<'de, D>(
    deserializer: D,
) -> Result<Option<Vec<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_trimmed_non_empty_vec(deserializer).map(Some)
}

pub(crate) fn deserialize_present_artifact_key<'de, D>(
    deserializer: D,
) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_artifact_key(deserializer).map(Some)
}

pub(crate) fn deserialize_present_input_path<'de, D>(
    deserializer: D,
) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = String::deserialize(deserializer)?;
    let trimmed = raw.trim();
    if is_valid_input_path(trimmed) {
        Ok(Some(trimmed.to_owned()))
    } else {
        Err(de::Error::custom(
            "expected a safe relative non-credential workspace path",
        ))
    }
}

pub(crate) fn deserialize_present_input_paths<'de, D>(
    deserializer: D,
) -> Result<Option<Vec<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    let items = Vec::<String>::deserialize(deserializer)?;
    items
        .into_iter()
        .map(|raw| {
            let trimmed = raw.trim();
            if is_valid_input_path(trimmed) {
                Ok(trimmed.to_owned())
            } else {
                Err(de::Error::custom(
                    "expected a safe relative non-credential workspace path",
                ))
            }
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

pub(crate) fn deserialize_present_non_git_artifact_key_vec<'de, D>(
    deserializer: D,
) -> Result<Option<Vec<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    let items = Vec::<String>::deserialize(deserializer)?;
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
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

pub(crate) fn deserialize_present_argv_command<'de, D>(
    deserializer: D,
) -> Result<Option<Vec<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_argv_command(deserializer).map(Some)
}

pub(crate) fn deserialize_present_argv_command_vec<'de, D>(
    deserializer: D,
) -> Result<Option<Vec<Vec<String>>>, D::Error>
where
    D: Deserializer<'de>,
{
    let cmds = Vec::<Vec<String>>::deserialize(deserializer)?;
    cmds.into_iter()
        .map(|argv| {
            let trimmed: Vec<String> = argv.into_iter().map(|s| s.trim().to_owned()).collect();
            if is_valid_argv_command(&trimmed) {
                Ok(trimmed)
            } else {
                Err(de::Error::custom(
                    "expected a valid argv-only command without shell wrappers or control operators",
                ))
            }
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}
