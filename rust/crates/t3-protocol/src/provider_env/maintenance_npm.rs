//! Batched global `npm` latest-version queries (`#40109`) and install-channel update prompt
//! suppression (`#39966`).

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;
use t3_contracts::ids::ProviderDriverKind;

/// Stable Homebrew cask update command for Claude Code.
pub const HOMEBREW_STABLE_CASK_UPDATE_COMMAND: &str = "brew upgrade --cask claude-code";

/// Command to switch Claude Code from the stable Homebrew cask to `@latest`.
pub const HOMEBREW_LATEST_CASK_SWITCH_COMMAND: &str =
    "brew uninstall --cask claude-code && brew install --cask claude-code@latest";

/// Returns `true` when `name` matches `NPM_IGNORED_VARIABLE` (`/^(?:CLAUDE|ELECTRON_RUN_AS_NODE$)/i`).
#[must_use]
pub fn is_ignored_npm_env_var(name: &str) -> bool {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return true;
    }
    let upper = trimmed.to_ascii_uppercase();
    upper.starts_with("CLAUDE") || upper == "ELECTRON_RUN_AS_NODE"
}

/// Filters an ambient environment for `npm` probe execution, stripping Claude/Electron vars.
#[must_use]
pub fn filter_npm_env<'a, I>(entries: I) -> BTreeMap<String, String>
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    let mut out = BTreeMap::new();
    for (key, value) in entries {
        if !is_ignored_npm_env_var(key) {
            out.insert(key.to_owned(), value.to_owned());
        }
    }
    out
}

/// Builds a deterministic single-flight cache key for `(npm_path, npm_prefix, filtered_env)`.
#[must_use]
pub fn npm_prefix_single_flight_key(
    npm_path: &str,
    npm_prefix: &str,
    filtered_env: &BTreeMap<String, String>,
) -> String {
    let mut key = format!("{npm_path}\0{npm_prefix}");
    for (k, v) in filtered_env {
        key.push('\0');
        key.push_str(k);
        key.push('=');
        key.push_str(v);
    }
    key
}

/// Batched `npm` query plan covering all installed provider packages under one global prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NpmBatchQueryPlan {
    /// Global `--prefix` passed to `npm`.
    pub prefix: String,
    /// Deduplicated sorted package names queried together.
    pub packages: Vec<String>,
    /// Arguments for `npm outdated -g --prefix <prefix> --json <packages...>`.
    pub outdated_args: Vec<String>,
}

impl NpmBatchQueryPlan {
    /// Builds arguments for `npm ls -g --prefix <prefix> --depth 0 --json <missing...>` to confirm
    /// up-to-date versions omitted from `npm outdated --json`.
    #[must_use]
    pub fn build_ls_args(&self, missing_packages: &[String]) -> Option<Vec<String>> {
        if missing_packages.is_empty() {
            return None;
        }
        let mut args = vec![
            "ls".to_owned(),
            "-g".to_owned(),
            "--prefix".to_owned(),
            self.prefix.clone(),
            "--depth".to_owned(),
            "0".to_owned(),
            "--json".to_owned(),
        ];
        args.extend(missing_packages.iter().cloned());
        Some(args)
    }
}

/// Plans one batched `npm outdated` query for all known provider packages under `npm_prefix`.
///
/// When `installed_packages` is `Some`, narrows the batch to registered packages present under
/// that prefix so `npm` never spends time resolving absent providers.
#[must_use]
pub fn plan_npm_prefix_batch_query(
    npm_prefix: &str,
    registered_packages: &[&str],
    installed_packages: Option<&[&str]>,
) -> Option<NpmBatchQueryPlan> {
    let installed_set: Option<BTreeSet<&str>> = installed_packages.map(|items| {
        items
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect()
    });
    let mut packages: BTreeSet<String> = BTreeSet::new();
    for raw in registered_packages {
        let pkg = raw.trim();
        if pkg.is_empty() {
            continue;
        }
        if let Some(ref installed) = installed_set
            && !installed.contains(pkg)
        {
            continue;
        }
        packages.insert(pkg.to_owned());
    }
    if packages.is_empty() {
        return None;
    }
    let ordered: Vec<String> = packages.into_iter().collect();
    let mut outdated_args = vec![
        "outdated".to_owned(),
        "-g".to_owned(),
        "--prefix".to_owned(),
        npm_prefix.to_owned(),
        "--json".to_owned(),
    ];
    outdated_args.extend(ordered.iter().cloned());
    Some(NpmBatchQueryPlan {
        prefix: npm_prefix.to_owned(),
        packages: ordered,
        outdated_args,
    })
}

/// Parses batched `npm outdated --json` stdout and maps each requested package to its `latest`.
#[must_use]
pub fn parse_npm_outdated_latest_versions(
    json_stdout: &str,
    requested: &[&str],
) -> BTreeMap<String, Option<String>> {
    let mut results: BTreeMap<String, Option<String>> =
        requested.iter().map(|p| ((*p).to_owned(), None)).collect();
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(json_stdout) else {
        return results;
    };
    for pkg in requested {
        if let Some(Value::Object(entry)) = map.get(*pkg)
            && let Some(latest) = entry.get("latest").and_then(Value::as_str)
        {
            let trimmed = latest.trim();
            if !trimmed.is_empty() {
                results.insert((*pkg).to_owned(), Some(trimmed.to_owned()));
            }
        }
    }
    results
}

/// Parses batched `npm ls --json` stdout and maps each requested package to its `version`.
#[must_use]
pub fn parse_npm_ls_versions(
    json_stdout: &str,
    requested: &[&str],
) -> BTreeMap<String, Option<String>> {
    let mut results: BTreeMap<String, Option<String>> =
        requested.iter().map(|p| ((*p).to_owned(), None)).collect();
    let Ok(Value::Object(root)) = serde_json::from_str::<Value>(json_stdout) else {
        return results;
    };
    let Some(Value::Object(deps)) = root.get("dependencies") else {
        return results;
    };
    for pkg in requested {
        if let Some(Value::Object(entry)) = deps.get(*pkg)
            && let Some(ver) = entry.get("version").and_then(Value::as_str)
        {
            let trimmed = ver.trim();
            if !trimmed.is_empty() {
                results.insert((*pkg).to_owned(), Some(trimmed.to_owned()));
            }
        }
    }
    results
}

/// Parsed semantic version (`major.minor.patch[-prerelease]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemverVersion {
    /// Major version number.
    pub major: u64,
    /// Minor version number.
    pub minor: u64,
    /// Patch version number.
    pub patch: u64,
    /// Prerelease identifiers if any.
    pub prerelease: Vec<String>,
}

/// Parses a `SemVer` string (allowing an optional leading `'v'` and optional `+build`).
#[must_use]
pub fn parse_semver(raw: &str) -> Option<SemverVersion> {
    let trimmed = raw.trim().strip_prefix('v').unwrap_or_else(|| raw.trim());
    let without_build = trimmed.split_once('+').map_or(trimmed, |(left, _)| left);
    let (core, pre) = match without_build.split_once('-') {
        Some((c, p)) if !p.is_empty() => (c, Some(p)),
        Some(_) => return None,
        None => (without_build, None),
    };
    let mut parts = core.split('.');
    let major = parts.next()?.parse::<u64>().ok()?;
    let minor = parts.next()?.parse::<u64>().ok()?;
    let patch = parts.next()?.parse::<u64>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    let prerelease = match pre {
        Some(p) => {
            let ids: Vec<String> = p.split('.').map(ToOwned::to_owned).collect();
            if ids.iter().any(String::is_empty) {
                return None;
            }
            ids
        }
        None => Vec::new(),
    };
    Some(SemverVersion {
        major,
        minor,
        patch,
        prerelease,
    })
}

/// Compares two parsed semantic versions according to `SemVer` 2.0 precedence.
#[must_use]
pub fn compare_semver(a: &SemverVersion, b: &SemverVersion) -> Ordering {
    match a.major.cmp(&b.major) {
        Ordering::Equal => {}
        ord => return ord,
    }
    match a.minor.cmp(&b.minor) {
        Ordering::Equal => {}
        ord => return ord,
    }
    match a.patch.cmp(&b.patch) {
        Ordering::Equal => {}
        ord => return ord,
    }
    match (a.prerelease.is_empty(), b.prerelease.is_empty()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        (false, false) => {
            for (left, right) in a.prerelease.iter().zip(&b.prerelease) {
                let ord = compare_pre_id(left, right);
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            a.prerelease.len().cmp(&b.prerelease.len())
        }
    }
}

fn compare_pre_id(a: &str, b: &str) -> Ordering {
    match (a.parse::<u64>(), b.parse::<u64>()) {
        (Ok(na), Ok(nb)) => na.cmp(&nb),
        (Ok(_), Err(_)) => Ordering::Less,
        (Err(_), Ok(_)) => Ordering::Greater,
        (Err(_), Err(_)) => a.cmp(b),
    }
}

/// Returns `Some(latest)` when `latest_version` and `min_version` both parse and `latest < min`.
#[must_use]
pub fn channel_latest_version_below<'a>(
    latest_version: Option<&'a str>,
    min_version: &str,
) -> Option<&'a str> {
    let latest = latest_version?;
    let parsed_latest = parse_semver(latest)?;
    let parsed_min = parse_semver(min_version)?;
    (compare_semver(&parsed_latest, &parsed_min) == Ordering::Less).then_some(latest)
}

/// Returns `true` when the install channel's `latest_version` lags behind the highest required
/// model `min_version`.
#[must_use]
pub fn provider_update_channel_lags(latest_version: Option<&str>, min_versions: &[&str]) -> bool {
    let Some(highest) = highest_min_version(min_versions) else {
        return false;
    };
    channel_latest_version_below(latest_version, highest).is_some()
}

/// Update prompt decision after inspecting the provider install channel's deliverable versions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelUpdatePromptOutcome {
    /// Prompt the user to update using the channel's normal update flow.
    OfferStandardUpdate,
    /// Suppress standard update prompt: Homebrew stable cask lags, guide switch to `@latest`.
    SuppressedSwitchHomebrewChannel {
        /// Latest version available on the stable cask.
        latest_version: String,
        /// Exact shell command to replace the stable cask with `claude-code@latest`.
        switch_command: &'static str,
    },
    /// Suppress standard update prompt: the install channel does not offer `min_version` yet.
    SuppressedChannelLags {
        /// Newest version currently offered by the install channel.
        latest_version: String,
    },
    /// Suppress standard update prompt: channel has no automatic update command (`!can_update`).
    SuppressedNoChannelUpdater,
}

/// Determines whether a provider update prompt should be shown or suppressed based on channel
/// capabilities (`#39966`).
#[must_use]
pub fn evaluate_channel_update_prompt(
    driver: &ProviderDriverKind,
    can_update: bool,
    update_command: Option<&str>,
    latest_version: Option<&str>,
    min_versions: &[&str],
) -> ChannelUpdatePromptOutcome {
    if let Some(highest_min) = highest_min_version(min_versions)
        && let Some(lagging) = channel_latest_version_below(latest_version, highest_min)
    {
        if driver.as_str() == "claudeAgent"
            && update_command == Some(HOMEBREW_STABLE_CASK_UPDATE_COMMAND)
        {
            return ChannelUpdatePromptOutcome::SuppressedSwitchHomebrewChannel {
                latest_version: lagging.to_owned(),
                switch_command: HOMEBREW_LATEST_CASK_SWITCH_COMMAND,
            };
        }
        return ChannelUpdatePromptOutcome::SuppressedChannelLags {
            latest_version: lagging.to_owned(),
        };
    }
    if !can_update || update_command.is_none_or(|c| c.trim().is_empty()) {
        return ChannelUpdatePromptOutcome::SuppressedNoChannelUpdater;
    }
    ChannelUpdatePromptOutcome::OfferStandardUpdate
}

fn highest_min_version<'a>(versions: &[&'a str]) -> Option<&'a str> {
    let mut best: Option<(&'a str, SemverVersion)> = None;
    for raw in versions {
        let Some(parsed) = parse_semver(raw) else {
            continue;
        };
        let replace = match &best {
            None => true,
            Some((_, current)) => compare_semver(&parsed, current) == Ordering::Greater,
        };
        if replace {
            best = Some((*raw, parsed));
        }
    }
    best.map(|(raw, _)| raw)
}
