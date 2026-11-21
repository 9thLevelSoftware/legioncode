//! Brand-aware base data directory and environment variable resolution.
//!
//! Routes home-directory env lookup and dotdir names through [`t3_contracts::brand::BrandConfig`].

use std::path::{Path, PathBuf};

use t3_contracts::brand::{ACTIVE_BRAND, BrandConfig};

/// Expands a leading `~` or `~/`/`~\` segment against `home_dir`.
#[must_use]
pub fn expand_home_dir(raw: &str, home_dir: Option<&Path>) -> PathBuf {
    let trimmed = raw.trim();
    if trimmed == "~"
        && let Some(home) = home_dir
    {
        return home.to_path_buf();
    }
    if let Some(rest) = trimmed
        .strip_prefix("~/")
        .or_else(|| trimmed.strip_prefix(r"~\"))
        && let Some(home) = home_dir
    {
        return home.join(rest);
    }
    PathBuf::from(trimmed)
}

/// Resolves the process OS home directory (`USERPROFILE` / `HOME`).
#[must_use]
pub fn os_user_home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .filter(|v| !v.is_empty())
        .or_else(|| std::env::var_os("HOME").filter(|v| !v.is_empty()))
        .map(PathBuf::from)
}

/// Resolves `(base_dir, base_dir_is_explicit)` for an explicit [`BrandConfig`] using `cli_base_dir`,
/// `brand.read_env_from("HOME", ...)`, or `home_dir.join(brand.home_dir_name)` (falling back to
/// `brand.fallback_home_dir_name` if that legacy directory already exists and the primary does not).
pub fn resolve_base_dir_for_brand(
    brand: &BrandConfig,
    cli_base_dir: Option<&Path>,
    home_dir: Option<&Path>,
    env_lookup: impl FnMut(&str) -> Option<String>,
) -> (PathBuf, bool) {
    if let Some(explicit) = cli_base_dir.filter(|p| !p.as_os_str().is_empty()) {
        return (expand_home_dir(&explicit.to_string_lossy(), home_dir), true);
    }
    if let Some(env_home) = brand.read_env_from("HOME", env_lookup) {
        return (expand_home_dir(&env_home, home_dir), true);
    }
    let root = home_dir.map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let primary = root.join(brand.home_dir_name);
    if brand.fallback_home_dir_name != brand.home_dir_name && !primary.exists() {
        let fallback = root.join(brand.fallback_home_dir_name);
        if fallback.exists() {
            return (fallback, false);
        }
    }
    (primary, false)
}

/// Resolves `(base_dir, base_dir_is_explicit)` for [`ACTIVE_BRAND`] using the current process environment.
#[must_use]
pub fn resolve_base_dir(cli_base_dir: Option<&Path>) -> (PathBuf, bool) {
    let home = os_user_home_dir();
    resolve_base_dir_for_brand(&ACTIVE_BRAND, cli_base_dir, home.as_deref(), |key| {
        std::env::var(key).ok()
    })
}
