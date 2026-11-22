//! Shared deserialization validators for managed policy documents.

use serde::{Deserialize, Deserializer, de};

pub(super) fn deserialize_schema_version_1<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: Deserializer<'de>,
{
    let version = u32::deserialize(deserializer)?;
    if version != 1 {
        return Err(de::Error::custom(format!(
            "unsupported schemaVersion {version}, expected 1"
        )));
    }
    Ok(version)
}

pub(super) fn deserialize_trimmed_non_empty<'de, D>(deserializer: D) -> Result<String, D::Error>
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

pub(super) fn deserialize_opt_trimmed_non_empty<'de, D>(
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

pub(super) fn deserialize_env_var_name<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = String::deserialize(deserializer)?;
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.chars().count() > 128 {
        return Err(de::Error::custom(
            "environment variable name must be 1..=128 characters",
        ));
    }
    let mut chars = trimmed.chars();
    let first_ok = chars
        .next()
        .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_');
    let rest_ok = chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_');
    if !(first_ok && rest_ok) {
        return Err(de::Error::custom(
            "environment variable name must match ^[a-zA-Z_][a-zA-Z0-9_]*$",
        ));
    }
    Ok(trimmed.to_owned())
}

pub(super) fn deserialize_opt_https_url<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let Some(trimmed) = deserialize_opt_trimmed_non_empty(deserializer)? else {
        return Ok(None);
    };
    let lower = trimmed.to_ascii_lowercase();
    let Some(host_and_rest) = lower.strip_prefix("https://") else {
        return Err(de::Error::custom("expected an https URL"));
    };
    let valid_host = host_and_rest
        .chars()
        .next()
        .is_some_and(|ch| !ch.is_whitespace() && ch != '/');
    if !valid_host {
        return Err(de::Error::custom("expected an https URL"));
    }
    Ok(Some(trimmed))
}

pub(super) fn deserialize_opt_non_empty_vec<'de, D, T>(
    deserializer: D,
) -> Result<Option<Vec<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    let opt = Option::<Vec<T>>::deserialize(deserializer)?;
    if let Some(ref vec) = opt
        && vec.is_empty()
    {
        return Err(de::Error::custom("array must not be empty"));
    }
    Ok(opt)
}
