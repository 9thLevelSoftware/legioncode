//! Peer link origin validation (`PeerLinks.ts:155-178` & `hostClassification.ts:76-88`).

use std::net::Ipv4Addr;

use crate::host_class::{is_local_loopback_host, is_tailnet_host, parse_ipv6};

use super::{PeerLinkError, PeerLinkErrorReason};

/// Validates and normalizes a peer base URL to its origin (`PeerLinks.ts:155-178`).
///
/// Plain `http://` is permitted ONLY for local loopback (`localhost`, `127.0.0.0/8`, `::1`) or
/// Tailscale hosts (`*.ts.net`, `fd7a:115c:a1e0::/48`); all other HTTP targets are rejected.
pub fn validate_peer_origin(url: &str) -> Result<String, PeerLinkError> {
    let trimmed = url.trim();
    let (scheme, rest) = if let Some(r) = trimmed.strip_prefix("https://") {
        ("https", r)
    } else if let Some(r) = trimmed.strip_prefix("http://") {
        ("http", r)
    } else if let Some((raw_scheme, r)) = trimmed.split_once("://") {
        if raw_scheme.eq_ignore_ascii_case("https") {
            ("https", r)
        } else if raw_scheme.eq_ignore_ascii_case("http") {
            ("http", r)
        } else {
            return Err(not_http_err(url));
        }
    } else {
        return Err(not_http_err(url));
    };

    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty()
        || authority
            .bytes()
            .any(|b| b.is_ascii_whitespace() || b == b'\\' || b < 0x20)
    {
        return Err(not_http_err(url));
    }
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, hp)| hp);
    let (normalized_host, display_host, port) = parse_authority_host_port(host_port, url)?;

    if scheme == "http"
        && !is_local_loopback_host(&normalized_host)
        && !is_tailnet_host(&normalized_host)
    {
        return Err(PeerLinkError::new(
            PeerLinkErrorReason::Unreachable,
            format!(
                "{url} is plain http, which would send the link's credentials unencrypted. Use its https address, or its Tailscale address."
            ),
        ));
    }

    let is_default_port = matches!((scheme, port), ("http", Some(80)) | ("https", Some(443)));
    match port {
        Some(p) if !is_default_port => Ok(format!("{scheme}://{display_host}:{p}")),
        _ => Ok(format!("{scheme}://{display_host}")),
    }
}

fn parse_authority_host_port(
    host_port: &str,
    original_url: &str,
) -> Result<(String, String, Option<u16>), PeerLinkError> {
    if host_port.starts_with('[') {
        let Some(close) = host_port.find(']') else {
            return Err(not_http_err(original_url));
        };
        let raw_ip = &host_port[1..close];
        let v6 = parse_ipv6(raw_ip).ok_or_else(|| not_http_err(original_url))?;
        let canonical = v6.to_string();
        let after = &host_port[close + 1..];
        let port = parse_optional_port(after, original_url)?;
        return Ok((canonical.clone(), format!("[{canonical}]"), port));
    }

    let (raw_host, port) = match host_port.rsplit_once(':') {
        Some((h, p)) => (h, parse_optional_port(&format!(":{p}"), original_url)?),
        None => (host_port, None),
    };
    if raw_host.is_empty() || raw_host.contains(':') {
        return Err(not_http_err(original_url));
    }
    let lower = raw_host.to_ascii_lowercase();
    let display = if let Ok(v4) = lower.parse::<Ipv4Addr>() {
        v4.to_string()
    } else {
        lower.clone()
    };
    Ok((display.clone(), display, port))
}

fn parse_optional_port(suffix: &str, original_url: &str) -> Result<Option<u16>, PeerLinkError> {
    if suffix.is_empty() {
        return Ok(None);
    }
    let Some(port_digits) = suffix.strip_prefix(':') else {
        return Err(not_http_err(original_url));
    };
    if port_digits.is_empty() || !port_digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(not_http_err(original_url));
    }
    let port = port_digits
        .parse::<u16>()
        .map_err(|_| not_http_err(original_url))?;
    Ok(Some(port))
}

fn not_http_err(url: &str) -> PeerLinkError {
    PeerLinkError::new(
        PeerLinkErrorReason::Unreachable,
        format!("{url} is not an http or https address."),
    )
}
