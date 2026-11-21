//! Local OpenSSH `config` (`Include` expansion) and `known_hosts` host discovery.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::SshError;
use super::config::{
    HostCondition, HostRule, RuleDirective, glob_matches_bytes, has_glob_wildcard, has_ssh_pattern,
    parse_ssh_config_line, resolve_configured_hostname, resolve_configured_port,
};
use super::known_hosts::parse_known_hosts;

/// Origin of a discovered SSH host entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SshHostSource {
    /// Discovered from `~/.ssh/config` or an included config file.
    SshConfig,
    /// Discovered from `~/.ssh/known_hosts`.
    KnownHosts,
}

/// SSH host candidate discovered on the local machine (`DesktopDiscoveredSshHost`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredSshHost {
    /// SSH alias or hostname to connect to.
    pub alias: String,
    /// Resolved target hostname.
    pub hostname: String,
    /// Optional username (`None` during discovery).
    pub username: Option<String>,
    /// Optional TCP port (`None` for config aliases; parsed port for `[host]:port` `known_hosts`).
    pub port: Option<u16>,
    /// Source file category (`"ssh-config"` or `"known-hosts"`).
    pub source: SshHostSource,
}

struct DiscoveryState {
    aliases: Vec<String>,
    alias_set: HashSet<String>,
    visited: HashSet<PathBuf>,
    rules: Vec<HostRule>,
}

/// Discovers SSH aliases from `<home_dir>/.ssh/config` (and `Include`s) plus `known_hosts`.
///
/// # Errors
///
/// Returns [`SshError::HostDiscovery`] if reading config or `known_hosts` files fails.
pub fn discover_ssh_hosts(home_dir: &Path) -> Result<Vec<DiscoveredSshHost>, SshError> {
    if home_dir.as_os_str().is_empty() {
        return Ok(Vec::new());
    }
    let ssh_dir = home_dir.join(".ssh");
    let mut state = DiscoveryState {
        aliases: Vec::new(),
        alias_set: HashSet::new(),
        visited: HashSet::new(),
        rules: Vec::new(),
    };
    collect_config_file(&ssh_dir.join("config"), &mut state, home_dir, &[]).map_err(|err| {
        SshError::HostDiscovery {
            message: format!("Failed to discover SSH hosts: {err}"),
        }
    })?;

    let known_path = ssh_dir.join("known_hosts");
    let known_hosts = if known_path.exists() {
        let raw = fs::read_to_string(&known_path).map_err(|err| SshError::HostDiscovery {
            message: format!("Failed to discover SSH hosts: {err}"),
        })?;
        parse_known_hosts(&raw)
    } else {
        Vec::new()
    };

    let mut discovered: BTreeMap<String, DiscoveredSshHost> = BTreeMap::new();
    let mut configured_names = HashSet::new();
    let mut configured_destinations = HashSet::new();
    for alias in &state.alias_set {
        let hostname = resolve_configured_hostname(alias, &state.rules);
        let port = resolve_configured_port(alias, &state.rules);
        let names: Vec<String> = match &hostname {
            Some(resolved) => vec![alias.clone(), resolved.clone()],
            None => vec![alias.clone()],
        };
        for name in names {
            let lower = name.to_ascii_lowercase();
            configured_names.insert(lower.clone());
            configured_destinations.insert(format!("{lower}\0{port}"));
        }
        discovered.insert(
            alias.clone(),
            DiscoveredSshHost {
                alias: alias.clone(),
                hostname: hostname.unwrap_or_else(|| alias.clone()),
                username: None,
                port: None,
                source: SshHostSource::SshConfig,
            },
        );
    }

    for entry in known_hosts {
        let lower = entry.hostname.to_ascii_lowercase();
        let covered = match entry.port {
            None => configured_names.contains(&lower),
            Some(p) => {
                configured_destinations.contains(&format!("{lower}\0{p}"))
                    || configured_destinations.contains(&format!("{lower}\0*"))
            }
        };
        if covered {
            continue;
        }
        let key = format!(
            "{}\0{}",
            entry.hostname,
            entry.port.map_or_else(String::new, |p| p.to_string())
        );
        discovered.entry(key).or_insert(DiscoveredSshHost {
            alias: entry.hostname.clone(),
            hostname: entry.hostname,
            username: None,
            port: entry.port,
            source: SshHostSource::KnownHosts,
        });
    }

    let mut out: Vec<DiscoveredSshHost> = discovered.into_values().collect();
    out.sort_by(|a, b| a.alias.cmp(&b.alias));
    Ok(out)
}

fn expand_home_path(input: &str, home_dir: &Path) -> String {
    let home = home_dir.to_string_lossy();
    if input == "~" {
        return home.into_owned();
    }
    if let Some(rest) = input
        .strip_prefix("~/")
        .or_else(|| input.strip_prefix("~\\"))
    {
        return PathBuf::from(home.as_ref())
            .join(rest)
            .to_string_lossy()
            .into_owned();
    }
    input.to_owned()
}

fn expand_glob(pattern: &Path) -> Result<Vec<PathBuf>, std::io::Error> {
    let pattern_str = pattern.to_string_lossy();
    if !has_glob_wildcard(&pattern_str) {
        return Ok(if pattern.exists() {
            vec![pattern.to_path_buf()]
        } else {
            Vec::new()
        });
    }
    let Some(dir_pat) = pattern.parent() else {
        return Ok(Vec::new());
    };
    let Some(base_pat) = pattern.file_name().and_then(|n| n.to_str()) else {
        return Ok(Vec::new());
    };
    let dir_is_glob = has_glob_wildcard(&dir_pat.to_string_lossy());
    let dirs = if dir_is_glob {
        expand_glob(dir_pat)?
    } else {
        vec![dir_pat.to_path_buf()]
    };

    let mut matched = Vec::new();
    for dir in dirs {
        if !dir.is_dir() {
            continue;
        }
        let entries = match fs::read_dir(&dir) {
            Ok(iter) => iter,
            Err(_) if dir_is_glob => continue,
            Err(err) => return Err(err),
        };
        for entry in entries.flatten() {
            let name_os = entry.file_name();
            let Some(name) = name_os.to_str() else {
                continue;
            };
            if name.starts_with('.') && !base_pat.starts_with('.') {
                continue;
            }
            if !glob_matches_bytes(base_pat.as_bytes(), name.as_bytes()) {
                continue;
            }
            let path = dir.join(name);
            if path.exists() {
                matched.push(path);
            }
        }
    }
    matched.sort_by(|a, b| {
        a.to_string_lossy()
            .as_bytes()
            .cmp(b.to_string_lossy().as_bytes())
    });
    Ok(matched)
}

fn collect_config_file(
    file_path: &Path,
    state: &mut DiscoveryState,
    home_dir: &Path,
    inherited: &[HostCondition],
) -> Result<(), std::io::Error> {
    let resolved = fs::canonicalize(file_path).unwrap_or_else(|_| file_path.to_path_buf());
    if state.visited.contains(&resolved) {
        state.rules.push(HostRule {
            conditions: inherited.to_vec(),
            directive: RuleDirective::Uncertain,
            value: None,
        });
        return Ok(());
    }
    if !resolved.exists() || resolved.is_dir() {
        return Ok(());
    }
    state.visited.insert(resolved.clone());

    let raw = fs::read_to_string(&resolved)?;
    let mut condition: HostCondition = Some(vec!["*".to_owned()]);
    for line in raw.lines() {
        let Some(parsed) = parse_ssh_config_line(line) else {
            continue;
        };
        let mut conditions = inherited.to_vec();
        conditions.push(condition.clone());
        match parsed.directive.as_str() {
            "host" => {
                condition = parsed.args;
                if let Some(items) = &condition {
                    for alias in items {
                        if !alias.is_empty()
                            && !has_ssh_pattern(alias)
                            && state.alias_set.insert(alias.clone())
                        {
                            state.aliases.push(alias.clone());
                        }
                    }
                }
            }
            "match" => condition = None,
            "hostname" => {
                let value = parsed
                    .args
                    .as_ref()
                    .filter(|a| a.len() == 1)
                    .map(|a| a[0].clone());
                state.rules.push(HostRule {
                    conditions,
                    directive: RuleDirective::Hostname,
                    value,
                });
            }
            "port" => {
                let value = parsed
                    .args
                    .as_ref()
                    .filter(|a| a.len() == 1)
                    .and_then(|a| a[0].parse::<u16>().ok())
                    .filter(|&p| p >= 1)
                    .map(|p| p.to_string());
                state.rules.push(HostRule {
                    conditions,
                    directive: RuleDirective::Port,
                    value,
                });
            }
            "include" => {
                handle_include_directive(parsed.args.as_deref(), state, home_dir, &conditions)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn handle_include_directive(
    patterns: Option<&[String]>,
    state: &mut DiscoveryState,
    home_dir: &Path,
    conditions: &[HostCondition],
) -> Result<(), std::io::Error> {
    let Some(patterns) = patterns else {
        state.rules.push(HostRule {
            conditions: conditions.to_vec(),
            directive: RuleDirective::Uncertain,
            value: None,
        });
        return Ok(());
    };
    for pat in patterns {
        let bad_tilde =
            pat.starts_with('~') && !pat.starts_with("~/") && !pat.starts_with("~\\") && pat != "~";
        if pat.contains('%') || pat.contains("${") || pat.contains('[') || bad_tilde {
            state.rules.push(HostRule {
                conditions: conditions.to_vec(),
                directive: RuleDirective::Uncertain,
                value: None,
            });
            continue;
        }
        let expanded = expand_home_path(pat, home_dir);
        let pat_path = Path::new(&expanded);
        let resolved_pat = if pat_path.is_absolute() {
            pat_path.to_path_buf()
        } else {
            home_dir.join(".ssh").join(pat_path)
        };
        for inc in expand_glob(&resolved_pat)? {
            collect_config_file(&inc, state, home_dir, conditions)?;
        }
    }
    Ok(())
}
