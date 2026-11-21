//! Git remote URL parsing and provider discovery.
//!
//! Parses `HTTPS`, `HTTP`, `ssh://`, `SCP`-style (`git@host:owner/repo.git`),
//! and `Azure` `DevOps` (`_git` / `:v3/`) remotes into structured `ParsedRemote`
//! records while rejecting local Windows drive paths and malformed targets.

use regex::Regex;
use std::sync::OnceLock;
use t3_contracts::ids::SourceControlProviderKind;

use crate::core::CoreError;

/// Supported git remote transport protocols.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RemoteTransport {
    /// Encrypted HTTP remote (`https://`).
    Https,
    /// Plaintext HTTP remote (`http://`).
    Http,
    /// Explicit SSH URL remote (`ssh://`).
    Ssh,
    /// `SCP`-style implicit SSH remote (`[user@]host:path`).
    Scp,
}

/// Decomposed components for an `Azure` `DevOps` remote repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AzureRemoteParts {
    /// `Azure` `DevOps` organization name.
    pub organization: String,
    /// `Azure` `DevOps` team project name.
    pub project: String,
    /// Repository name inside the project.
    pub repository: String,
}

/// Normalized representation of a parsed git remote URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedRemote {
    /// Trimmed original remote string.
    pub raw: String,
    /// Detected transport mechanism.
    pub transport: RemoteTransport,
    /// Lowercased normalized remote hostname.
    pub host: String,
    /// Optional explicit port from `https://` or `ssh://` URLs.
    pub port: Option<u16>,
    /// Owner, group path, or `<org>/<project>` namespace.
    pub owner: String,
    /// Repository slug with any trailing `.git` stripped.
    pub repo: String,
    /// Canonical `<owner>/<repo>` full repository path.
    pub full_name: String,
    /// `Azure` `DevOps` structured components when detected.
    pub azure: Option<AzureRemoteParts>,
    /// Best-effort provider identifier derived from the normalized hostname.
    pub provider_hint: Option<SourceControlProviderKind>,
}

fn scp_remote_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let compiled = Regex::new(r"^(?:(?P<user>[^@/\s]+)@)?(?P<host>[^:/\s]+):(?P<path>.+)$");
        // INFALLIBLE: static SCP remote regex literal is verified at compile/test time.
        compiled.expect("valid SCP remote regex")
    })
}

/// Returns `true` when `raw` looks like a Windows drive path (`C:\...` or `D:/...`).
///
/// ```
/// use t3_scm::core::discovery::is_windows_drive_path;
///
/// assert!(is_windows_drive_path(r"C:\src\repo"));
/// assert!(is_windows_drive_path("D:/work/repo.git"));
/// assert!(!is_windows_drive_path("git@github.com:org/repo.git"));
/// ```
#[must_use]
pub fn is_windows_drive_path(raw: &str) -> bool {
    let bytes = raw.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
}

/// Normalizes a hostname by trimming whitespace, stripping brackets, and lowercasing.
#[must_use]
pub fn normalize_git_host(raw_host: &str) -> Option<String> {
    let trimmed = raw_host
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']');
    if trimmed.is_empty()
        || trimmed
            .bytes()
            .any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
    {
        return None;
    }
    Some(trimmed.to_ascii_lowercase())
}

/// Infers a `SourceControlProviderKind` identifier from well-known hostnames.
#[must_use]
pub fn detect_provider_from_host(host: &str) -> Option<SourceControlProviderKind> {
    let normalized = normalize_git_host(host)?;
    let kind_str = if normalized == "github.com" || normalized == "api.github.com" {
        "github"
    } else if normalized == "gitlab.com" || normalized.starts_with("gitlab.") {
        "gitlab"
    } else if normalized == "dev.azure.com"
        || normalized == "ssh.dev.azure.com"
        || normalized.ends_with(".visualstudio.com")
    {
        "azure-devops"
    } else if normalized == "bitbucket.org" || normalized == "api.bitbucket.org" {
        "bitbucket"
    } else if normalized == "git.cafe" || normalized == "staging.git.cafe" {
        "gitcafe"
    } else if normalized == "origin.cursor.com" || normalized == "api.cursor.com" {
        "cursor-origin"
    } else if normalized.contains("forgejo") || normalized.contains("codeberg.org") {
        "forgejo"
    } else {
        return None;
    };
    SourceControlProviderKind::new(kind_str).ok()
}

/// Parses an `HTTPS`, `HTTP`, `ssh://`, or `SCP` git remote into a [`ParsedRemote`].
///
/// Supports standard `owner/repo`, multi-segment `GitLab` subgroups, and `Azure`
/// `DevOps` `_git` / `v3` paths.
///
/// # Errors
///
/// Returns [`CoreError::InvalidRemote`] if `raw` is empty, refers to a local
/// filesystem or Windows drive path, omits host/repo components, or uses an
/// unsupported URI scheme.
///
/// ```
/// use t3_scm::core::discovery::{parse_git_remote_url, RemoteTransport};
///
/// let scp = parse_git_remote_url("git@github.com:t3dotgg/t3code.git")?;
/// assert_eq!(scp.transport, RemoteTransport::Scp);
/// assert_eq!(scp.host, "github.com");
/// assert_eq!(scp.owner, "t3dotgg");
/// assert_eq!(scp.repo, "t3code");
/// assert_eq!(scp.full_name, "t3dotgg/t3code");
///
/// let az = parse_git_remote_url("https://dev.azure.com/myorg/myproj/_git/myrepo")?;
/// assert_eq!(az.azure.as_ref().map(|a| a.repository.as_str()), Some("myrepo"));
/// assert_eq!(az.owner, "myorg/myproj");
/// # Ok::<(), t3_scm::core::CoreError>(())
/// ```
pub fn parse_git_remote_url(raw: &str) -> Result<ParsedRemote, CoreError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(invalid_remote(raw, "remote URL is empty"));
    }
    if is_windows_drive_path(trimmed) || trimmed.starts_with('/') || trimmed.starts_with("file://")
    {
        return Err(invalid_remote(raw, "local paths are not remote URLs"));
    }

    if let Some(rest) = trimmed.strip_prefix("https://") {
        return parse_scheme_url(trimmed, RemoteTransport::Https, rest);
    }
    if let Some(rest) = trimmed.strip_prefix("http://") {
        return parse_scheme_url(trimmed, RemoteTransport::Http, rest);
    }
    if let Some(rest) = trimmed.strip_prefix("ssh://") {
        return parse_scheme_url(trimmed, RemoteTransport::Ssh, rest);
    }
    if trimmed.contains("://") {
        return Err(invalid_remote(raw, "unsupported URL scheme"));
    }

    let Some(caps) = scp_remote_regex().captures(trimmed) else {
        return Err(invalid_remote(raw, "unrecognized SCP remote format"));
    };
    let raw_host = caps.name("host").map_or("", |m| m.as_str());
    let raw_path = caps.name("path").map_or("", |m| m.as_str());
    let host = normalize_git_host(raw_host)
        .ok_or_else(|| invalid_remote(raw, "invalid host in SCP remote"))?;

    build_parsed_remote(trimmed, RemoteTransport::Scp, host, None, raw_path)
}

fn parse_scheme_url(
    original: &str,
    transport: RemoteTransport,
    without_scheme: &str,
) -> Result<ParsedRemote, CoreError> {
    let without_query = without_scheme
        .split(['?', '#'])
        .next()
        .unwrap_or(without_scheme);
    let slash_idx = without_query
        .find('/')
        .ok_or_else(|| invalid_remote(original, "remote URL is missing repository path"))?;
    let authority = &without_query[..slash_idx];
    let path_part = &without_query[slash_idx + 1..];

    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    let (host, port) = split_host_and_port(host_port)
        .ok_or_else(|| invalid_remote(original, "invalid host or port in URL authority"))?;

    build_parsed_remote(original, transport, host, port, path_part)
}

fn split_host_and_port(host_port: &str) -> Option<(String, Option<u16>)> {
    if host_port.starts_with('[') {
        let close_idx = host_port.find(']')?;
        let host = normalize_git_host(&host_port[..=close_idx])?;
        let tail = &host_port[close_idx + 1..];
        if tail.is_empty() {
            return Some((host, None));
        }
        let port_str = tail.strip_prefix(':')?;
        let port = port_str.parse::<u16>().ok()?;
        return Some((host, Some(port)));
    }

    if let Some((h, p)) = host_port.rsplit_once(':') {
        let host = normalize_git_host(h)?;
        let port = p.parse::<u16>().ok()?;
        return Some((host, Some(port)));
    }

    let host = normalize_git_host(host_port)?;
    Some((host, None))
}

fn build_parsed_remote(
    original: &str,
    transport: RemoteTransport,
    host: String,
    port: Option<u16>,
    raw_path: &str,
) -> Result<ParsedRemote, CoreError> {
    let clean_path = raw_path
        .trim_start_matches('/')
        .trim_end_matches('/')
        .strip_suffix(".git")
        .unwrap_or_else(|| raw_path.trim_start_matches('/').trim_end_matches('/'));
    let segments: Vec<&str> = clean_path
        .split('/')
        .filter(|seg| !seg.is_empty())
        .collect();

    if segments.len() < 2 || segments.contains(&"..") || segments.contains(&".") {
        return Err(invalid_remote(
            original,
            "remote path must contain at least owner and repository",
        ));
    }

    let provider_hint = detect_provider_from_host(&host);
    if let Some((azure, owner, repo)) = parse_azure_segments(&host, &segments) {
        let full_name = format!("{owner}/{repo}");
        return Ok(ParsedRemote {
            raw: original.to_owned(),
            transport,
            host,
            port,
            owner,
            repo,
            full_name,
            azure: Some(azure),
            provider_hint,
        });
    }

    let repo = (*segments.last().unwrap_or(&"")).to_owned();
    let owner = segments[..segments.len() - 1].join("/");
    let full_name = format!("{owner}/{repo}");

    Ok(ParsedRemote {
        raw: original.to_owned(),
        transport,
        host,
        port,
        owner,
        repo,
        full_name,
        azure: None,
        provider_hint,
    })
}

fn azure_tuple(org: &str, proj: &str, repo: &str) -> (AzureRemoteParts, String, String) {
    (
        AzureRemoteParts {
            organization: org.to_owned(),
            project: proj.to_owned(),
            repository: repo.to_owned(),
        },
        format!("{org}/{proj}"),
        repo.to_owned(),
    )
}

fn parse_azure_segments(
    host: &str,
    segments: &[&str],
) -> Option<(AzureRemoteParts, String, String)> {
    // SSH Azure URL format: v3/<org>/<project>/<repo>
    if segments.len() == 4 && segments[0] == "v3" && host == "ssh.dev.azure.com" {
        return Some(azure_tuple(segments[1], segments[2], segments[3]));
    }
    // HTTPS Azure URL format on dev.azure.com: <org>/<project>/_git/<repo>
    if segments.len() == 4 && segments[2] == "_git" {
        return Some(azure_tuple(segments[0], segments[1], segments[3]));
    }
    // VisualStudio legacy format: https://<org>.visualstudio.com/[DefaultCollection/]<project>/_git/<repo>
    if let Some(org_sub) = host.strip_suffix(".visualstudio.com") {
        let git_pos = segments.iter().position(|&s| s == "_git")?;
        if git_pos >= 1 && git_pos + 1 < segments.len() {
            return Some(azure_tuple(
                org_sub,
                segments[git_pos - 1],
                segments[git_pos + 1],
            ));
        }
    }
    None
}

fn invalid_remote(raw: &str, reason: &str) -> CoreError {
    CoreError::InvalidRemote {
        raw: raw.to_owned(),
        reason: reason.to_owned(),
    }
}
