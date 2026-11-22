//! Hostname and IP literal classification matching `packages/shared/src/hostClassification.ts`.

use std::net::{Ipv4Addr, Ipv6Addr};

/// Normalizes a hostname or bracketed IPv6 literal (`hostClassification.ts:1-5`).
pub fn normalize_hostname(host: &str) -> String {
    let trimmed = host.trim().to_ascii_lowercase();
    let unbracketed = trimmed
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(&trimmed);
    unbracketed.trim_end_matches('.').to_owned()
}

/// Parses an IPv4 dotted-quad literal strictly (`0..=255`, no leading/extra octets).
pub(crate) fn parse_ipv4(host: &str) -> Option<Ipv4Addr> {
    let normalized = normalize_hostname(host);
    normalized.parse::<Ipv4Addr>().ok()
}

/// Parses an IPv6 literal (without brackets) into [`Ipv6Addr`].
pub(crate) fn parse_ipv6(host: &str) -> Option<Ipv6Addr> {
    let normalized = normalize_hostname(host);
    if !normalized.contains(':') {
        return None;
    }
    normalized.parse::<Ipv6Addr>().ok()
}

/// Parses an IPv4-mapped IPv6 literal (`::ffff:127.0.0.1` or `::ffff:7f00:1`).
pub(crate) fn parse_ipv4_mapped_ipv6(host: &str) -> Option<Ipv4Addr> {
    let ipv6 = parse_ipv6(host)?;
    ipv6.to_ipv4_mapped()
}

/// Returns `true` for `localhost`, `::1`, and `127.0.0.0/8` (`hostClassification.ts:76-80`).
pub fn is_local_loopback_host(host: &str) -> bool {
    let normalized = normalize_hostname(host);
    if normalized == "localhost" || normalized == "::1" {
        return true;
    }
    if let Some(v6) = parse_ipv6(&normalized)
        && v6.is_loopback()
    {
        return true;
    }
    parse_ipv4(&normalized).is_some_and(|v4| v4.octets()[0] == 127)
}

/// Returns `true` for `MagicDNS` (`*.ts.net`) or Tailscale ULA `fd7a:115c:a1e0::/48`.
///
/// Matches `hostClassification.ts:82-88`: shared `100.64.0.0/10` addresses alone return `false`.
pub fn is_tailnet_host(host: &str) -> bool {
    let normalized = normalize_hostname(host);
    if normalized.ends_with(".ts.net") {
        return true;
    }
    parse_ipv6(&normalized).is_some_and(|v6| {
        let segs = v6.segments();
        segs[0] == 0xfd7a && segs[1] == 0x115c && segs[2] == 0xa1e0
    })
}

/// Returns `true` for Tailscale / CGNAT IPv4 addresses in `100.64.0.0/10` (`tailscale.ts:197-213`).
pub fn is_tailscale_ipv4_address(address: &str) -> bool {
    let Ok(ip) = address.parse::<Ipv4Addr>() else {
        return false;
    };
    let [first, second, _, _] = ip.octets();
    first == 100 && (64..=127).contains(&second)
}
