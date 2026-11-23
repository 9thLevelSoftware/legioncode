//! Tailscale `serve status --json` route parsing and CLI argument builders.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::host_class::{normalize_hostname, parse_ipv4_mapped_ipv6, parse_ipv6};

use super::DEFAULT_TAILSCALE_SERVE_PORT;

/// Target loopback host and port that a `LegionCode` server binds to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServeTarget {
    /// Bound host (`"127.0.0.1"`, `"::1"`, etc.).
    pub host: String,
    /// Bound TCP port.
    pub port: u16,
}

/// A Tailscale Serve route forwarding to a local server (`tailscale.ts:488-497`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TailscaleServeRoute {
    /// Tailnet-facing Serve port.
    pub serve_port: u16,
    /// Web mount path (e.g. `"/"`), or `None` for a TCP forward.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mount: Option<String>,
    /// Machine hostname for web routes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// Target host string when resolution could reach another loopback service.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ambiguous: Option<String>,
    /// Owner scope (`"svc:..."` or `"foreground"`), or `None` for node root config.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
}

/// Builds `tailscale serve` arguments to publish `http://<local_host>:<local_port>` on `serve_port`.
#[must_use]
pub fn ensure_tailscale_serve_args(
    local_port: u16,
    serve_port: Option<u16>,
    local_host: Option<&str>,
) -> Vec<String> {
    let port = serve_port.unwrap_or(DEFAULT_TAILSCALE_SERVE_PORT);
    let host = local_host.unwrap_or("127.0.0.1");
    vec![
        "serve".to_owned(),
        "--bg".to_owned(),
        format!("--https={port}"),
        format!("http://{host}:{local_port}"),
    ]
}

/// Builds `tailscale serve ... off` arguments for `route`, or `None` for foreground routes (`tailscale.ts:504-516`).
#[must_use]
pub fn tailscale_serve_off_args(route: &TailscaleServeRoute) -> Option<Vec<String>> {
    if route.owner.as_deref() == Some("foreground") {
        return None;
    }
    let mut args = vec!["serve".to_owned()];
    if let Some(owner) = &route.owner {
        args.push(format!("--service={owner}"));
    }
    match &route.mount {
        None => args.push(format!("--tcp={}", route.serve_port)),
        Some(mount) => {
            args.push(format!("--https={}", route.serve_port));
            args.push(format!("--set-path={mount}"));
        }
    }
    args.push("off".to_owned());
    Some(args)
}

#[derive(Deserialize)]
pub(super) struct ServeTcpEntry {
    #[serde(rename = "TCPForward")]
    pub(super) tcp_forward: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct ServeWebHandler {
    #[serde(rename = "Proxy")]
    pub(super) proxy: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct ServeWebSite {
    #[serde(rename = "Handlers")]
    pub(super) handlers: Option<BTreeMap<String, ServeWebHandler>>,
}

#[derive(Deserialize)]
pub(super) struct ServeConfigJson {
    #[serde(rename = "TCP")]
    pub(super) tcp: Option<BTreeMap<String, ServeTcpEntry>>,
    #[serde(rename = "Web")]
    pub(super) web: Option<BTreeMap<String, ServeWebSite>>,
}

#[derive(Deserialize)]
pub(super) struct ServeStatusJson {
    #[serde(flatten)]
    pub(super) root: ServeConfigJson,
    #[serde(rename = "Services")]
    pub(super) services: Option<BTreeMap<String, ServeConfigJson>>,
    #[serde(rename = "Foreground")]
    pub(super) foreground: Option<BTreeMap<String, ServeConfigJson>>,
}

/// Parses `tailscale serve status --json` routes forwarding to `target`, or `None` on invalid JSON.
#[must_use]
pub fn parse_tailscale_serve_routes_to(
    raw_serve_status_json: &str,
    target: &ServeTarget,
) -> Option<Vec<TailscaleServeRoute>> {
    let parsed: Option<ServeStatusJson> = serde_json::from_str(raw_serve_status_json).ok()?;
    let Some(status) = parsed else {
        return Some(Vec::new());
    };
    let mut routes = Vec::new();
    collect_routes_in(&status.root, None, target, &mut routes);
    if let Some(services) = &status.services {
        for (name, svc) in services {
            collect_routes_in(svc, Some(name.as_str()), target, &mut routes);
        }
    }
    if let Some(foreground) = &status.foreground {
        for session in foreground.values() {
            collect_routes_in(session, Some("foreground"), target, &mut routes);
        }
    }
    Some(routes)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Reach {
    Exact,
    Either,
}

fn reaches(hostname: &str, bind_host: &str) -> Option<Reach> {
    let mut address = normalize_hostname(hostname);
    if let Some(v4) = parse_ipv4_mapped_ipv6(&address) {
        address = v4.to_string();
    } else if let Some(v6) = parse_ipv6(&address) {
        address = v6.to_string();
    }
    let is_literal = address.contains(':') || address.parse::<std::net::Ipv4Addr>().is_ok();
    if !is_literal {
        return Some(Reach::Either);
    }
    if address == "::" {
        return (bind_host == "127.0.0.1" || bind_host == "::1").then_some(Reach::Either);
    }
    let mapped = if address == "0.0.0.0" {
        "127.0.0.1"
    } else {
        address.as_str()
    };
    (mapped == bind_host).then_some(Reach::Exact)
}

pub(super) fn expand_proxy(proxy: &str) -> String {
    if !proxy.is_empty() && proxy.bytes().all(|b| b.is_ascii_digit()) {
        format!("http://127.0.0.1:{proxy}")
    } else if let Some(rest) = proxy.strip_prefix("https+insecure://") {
        format!("https://{rest}")
    } else if proxy.starts_with("http://")
        || proxy.starts_with("https://")
        || proxy.starts_with("unix:")
    {
        proxy.to_owned()
    } else {
        format!("http://{proxy}")
    }
}

pub(super) fn dialed_host_port(url: &str) -> Option<(String, u16)> {
    let (scheme_lower, rest) = if let Some(r) = url.strip_prefix("http://") {
        ("http", r)
    } else if let Some(r) = url.strip_prefix("https://") {
        ("https", r)
    } else {
        let r = url.strip_prefix("tcp://")?;
        ("tcp", r)
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, hp)| hp);
    let (host, port_str) = if host_port.starts_with('[') {
        let close = host_port.find(']')?;
        let host = &host_port[..=close];
        let after = &host_port[close + 1..];
        let port = after.strip_prefix(':').unwrap_or("");
        (host, port)
    } else if let Some((h, p)) = host_port.rsplit_once(':')
        && p.bytes().all(|b| b.is_ascii_digit())
    {
        (h, p)
    } else {
        (host_port, "")
    };
    let port = if port_str.is_empty() {
        if scheme_lower == "https" { 443 } else { 80 }
    } else {
        port_str.parse::<u16>().ok()?
    };
    Some((host.to_owned(), port))
}

pub(super) fn reaches_target(url: &str, target: &ServeTarget) -> Option<(Reach, String)> {
    let (host, port) = dialed_host_port(url)?;
    if port != target.port {
        return None;
    }
    let reach = reaches(&host, &target.host)?;
    Some((reach, host))
}

fn collect_routes_in(
    config: &ServeConfigJson,
    owner: Option<&str>,
    target: &ServeTarget,
    out: &mut Vec<TailscaleServeRoute>,
) {
    if let Some(tcp) = &config.tcp {
        for (port_str, handler) in tcp {
            let Some(forward) = &handler.tcp_forward else {
                continue;
            };
            if forward.starts_with("unix:") {
                continue;
            }
            let Ok(serve_port) = port_str.parse::<u16>() else {
                continue;
            };
            let Some((reach, host)) = reaches_target(&format!("tcp://{forward}"), target) else {
                continue;
            };
            out.push(TailscaleServeRoute {
                serve_port,
                mount: None,
                host: None,
                ambiguous: (reach == Reach::Either).then_some(host),
                owner: owner.map(str::to_owned),
            });
        }
    }
    if let Some(web) = &config.web {
        for (host_port, site) in web {
            let Some((host, port_str)) = host_port.rsplit_once(':') else {
                continue;
            };
            let Ok(serve_port) = port_str.parse::<u16>() else {
                continue;
            };
            let Some(handlers) = &site.handlers else {
                continue;
            };
            for (mount, handler) in handlers {
                let Some(proxy) = &handler.proxy else {
                    continue;
                };
                let Some((reach, dialed_host)) = reaches_target(&expand_proxy(proxy), target)
                else {
                    continue;
                };
                out.push(TailscaleServeRoute {
                    serve_port,
                    mount: Some(mount.clone()),
                    host: Some(host.to_owned()),
                    ambiguous: (reach == Reach::Either).then_some(dialed_host),
                    owner: owner.map(str::to_owned),
                });
            }
        }
    }
}
