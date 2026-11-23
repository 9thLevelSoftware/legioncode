//! Outbound main-window request header stripping so remote servers behind origin-checking proxies
//! do not reject Desktop with `403` (`#41092`).

use std::net::IpAddr;

/// Policy governing when Desktop removes its synthetic custom-scheme `Origin` and `Sec-Fetch-*`
/// headers from outbound requests (`apps/desktop/src/window/RemoteRequestHeaders.ts`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MainWindowRequestHeaderPolicy {
    pub desktop_origin: String,
    pub main_web_contents_id: u64,
    pub clerk_frontend_api_hostname: Option<String>,
    pub dev_server_origin: Option<String>,
    pub is_development: bool,
}

/// Outbound request details inspected by [`strip_desktop_origin_headers`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MainWindowRequestHeaderDetails {
    pub url: String,
    pub web_contents_id: Option<u64>,
    pub is_subframe: bool,
    pub request_headers: Vec<(String, String)>,
}

/// Strips synthetic Desktop `Origin` and `Sec-Fetch-*` headers on outbound main-window HTTP/WS
/// requests unless targeting Clerk or a dev/loopback server (`#41092`).
pub fn strip_desktop_origin_headers(
    policy: &MainWindowRequestHeaderPolicy,
    details: &MainWindowRequestHeaderDetails,
) -> Option<Vec<(String, String)>> {
    if details.web_contents_id != Some(policy.main_web_contents_id) || details.is_subframe {
        return None;
    }
    if !has_exact_desktop_origin_header(&details.request_headers, &policy.desktop_origin) {
        return None;
    }
    let parsed = parse_http_or_ws_url(&details.url)?;
    if should_preserve_origin_header(policy, &parsed) {
        return None;
    }
    Some(
        details
            .request_headers
            .iter()
            .filter(|(name, _)| {
                let lower = name.to_ascii_lowercase();
                lower != "origin" && !lower.starts_with("sec-fetch-")
            })
            .cloned()
            .collect(),
    )
}

#[derive(Debug)]
struct ParsedUrlTarget {
    http_origin: String,
    hostname: String,
}

fn parse_http_or_ws_url(raw_url: &str) -> Option<ParsedUrlTarget> {
    let (scheme_raw, rest) = raw_url.split_once("://")?;
    let scheme = scheme_raw.to_ascii_lowercase();
    let http_scheme = match scheme.as_str() {
        "http" | "ws" => "http",
        "https" | "wss" => "https",
        _ => return None,
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.is_empty() {
        return None;
    }
    let host_port = authority
        .rsplit_once('@')
        .map_or(authority, |(_, right)| right);
    let hostname = extract_hostname(host_port)?;
    Some(ParsedUrlTarget {
        http_origin: format!("{http_scheme}://{host_port}"),
        hostname: normalize_hostname(hostname),
    })
}

fn extract_hostname(host_port: &str) -> Option<&str> {
    if let Some(after_open) = host_port.strip_prefix('[') {
        let (ipv6, _) = after_open.split_once(']')?;
        return (!ipv6.is_empty()).then_some(ipv6);
    }
    let host = host_port.split(':').next().unwrap_or_default();
    (!host.is_empty()).then_some(host)
}

fn normalize_hostname(hostname: &str) -> String {
    let lower = hostname.to_ascii_lowercase();
    let unbracketed = lower
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(&lower);
    unbracketed.trim_end_matches('.').to_owned()
}

fn is_loopback_hostname(hostname: &str) -> bool {
    hostname == "localhost"
        || hostname.ends_with(".localhost")
        || hostname
            .parse::<IpAddr>()
            .is_ok_and(|addr| addr.is_loopback())
}

fn has_exact_desktop_origin_header(headers: &[(String, String)], desktop_origin: &str) -> bool {
    let mut matched = false;
    for (name, value) in headers {
        if !name.eq_ignore_ascii_case("origin") {
            continue;
        }
        if value != desktop_origin {
            return false;
        }
        matched = true;
    }
    matched
}

fn should_preserve_origin_header(
    policy: &MainWindowRequestHeaderPolicy,
    url: &ParsedUrlTarget,
) -> bool {
    if policy
        .clerk_frontend_api_hostname
        .as_deref()
        .is_some_and(|clerk| url.hostname == normalize_hostname(clerk))
    {
        return true;
    }
    if !policy.is_development {
        return false;
    }
    if policy
        .dev_server_origin
        .as_deref()
        .is_some_and(|dev_origin| url.http_origin == dev_origin)
    {
        return true;
    }
    is_loopback_hostname(&url.hostname)
}
