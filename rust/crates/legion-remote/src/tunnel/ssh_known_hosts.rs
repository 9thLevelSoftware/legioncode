//! OpenSSH `known_hosts` parsing helpers (`config.ts:299-350`).

use std::collections::BTreeMap;

use super::config::has_ssh_pattern;

/// Parsed `~/.ssh/known_hosts` entry (`config.ts:299-302`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct KnownHost {
    /// Hostname or IP literal.
    pub hostname: String,
    /// Optional bracketed port (`[host]:2222`).
    pub port: Option<u16>,
}

/// Parses `~/.ssh/known_hosts` text, skipping `@markers`, comments, hashed entries, and wildcards.
#[must_use]
pub fn parse_known_hosts(raw: &str) -> Vec<KnownHost> {
    let mut hosts: BTreeMap<(String, u16), KnownHost> = BTreeMap::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let mut fields = trimmed.split_whitespace();
        let first = fields.next().unwrap_or("");
        let host_field = if first.starts_with('@') {
            fields.next().unwrap_or("")
        } else {
            first
        };
        if host_field.is_empty() || host_field.starts_with('|') {
            continue;
        }
        for raw_host in host_field.split(',') {
            let Some(parsed) = parse_known_hosts_host(raw_host.trim()) else {
                continue;
            };
            let hostname = parsed.hostname.trim().to_owned();
            if hostname.is_empty() || has_ssh_pattern(&hostname) {
                continue;
            }
            let key = (hostname.clone(), parsed.port.unwrap_or(0));
            hosts.entry(key).or_insert(KnownHost {
                hostname,
                port: parsed.port,
            });
        }
    }
    hosts.into_values().collect()
}

fn parse_known_hosts_host(raw_host: &str) -> Option<KnownHost> {
    if let Some(after_open) = raw_host.strip_prefix('[')
        && let Some((host, port_str)) = after_open.split_once("]:")
        && !host.is_empty()
    {
        let port = port_str.parse::<u16>().ok()?;
        return (port >= 1).then(|| KnownHost {
            hostname: host.to_owned(),
            port: Some(port),
        });
    }
    if !raw_host.contains(':') {
        return Some(KnownHost {
            hostname: raw_host.to_owned(),
            port: None,
        });
    }
    let first = raw_host.find(':');
    let last = raw_host.rfind(':');
    let hostname = if first == last {
        raw_host[..last.unwrap_or(raw_host.len())].to_owned()
    } else {
        raw_host.to_owned()
    };
    Some(KnownHost {
        hostname,
        port: None,
    })
}
