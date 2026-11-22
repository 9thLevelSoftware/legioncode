//! Tailscale Serve root (`/`) holder inspection (`tailscale.ts:690-760`).

use crate::host_class::{is_local_loopback_host, normalize_hostname};

use super::serve::{ServeStatusJson, ServeTarget, dialed_host_port, expand_proxy, reaches_target};

/// A handler holding `/` on a Serve port (`tailscale.ts:690-692`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TailscaleServeRootHolder {
    /// Root handler forwards to our target server.
    Ours,
    /// Root handler belongs to another service, optionally naming its loopback listener.
    Other {
        /// Loopback listener target when known and local.
        loopback: Option<ServeTarget>,
    },
}

/// Classification of who holds `/` on a Tailscale Serve port (`tailscale.ts:747-760`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TailscaleServeRootStatus {
    /// No handler exists at `/`.
    None,
    /// Our server already owns `/`.
    Ours,
    /// An orphaned loopback port holds `/` with no live listener.
    Dead,
    /// Another live server or non-loopback handler owns `/`.
    Other,
}

/// Parses root (`/`) holders on `serve_port` outside Tailscale Services (`tailscale.ts:698-728`).
#[must_use]
pub fn parse_tailscale_serve_root_holders(
    raw_serve_status_json: &str,
    serve_port: u16,
    target: &ServeTarget,
) -> Option<Vec<TailscaleServeRootHolder>> {
    let parsed: Option<ServeStatusJson> = serde_json::from_str(raw_serve_status_json).ok()?;
    let Some(status) = parsed else {
        return Some(Vec::new());
    };
    let mut holders = Vec::new();
    if let Some(tcp) = &status.root.tcp
        && tcp
            .get(&serve_port.to_string())
            .and_then(|e| e.tcp_forward.as_ref())
            .is_some()
    {
        holders.push(TailscaleServeRootHolder::Other { loopback: None });
    }
    if let Some(web) = &status.root.web {
        for (host_port, site) in web {
            let Some((_, port_str)) = host_port.rsplit_once(':') else {
                continue;
            };
            if port_str.parse::<u16>().ok() != Some(serve_port) {
                continue;
            }
            let Some(root) = site.handlers.as_ref().and_then(|h| h.get("/")) else {
                continue;
            };
            let Some(proxy) = &root.proxy else {
                holders.push(TailscaleServeRootHolder::Other { loopback: None });
                continue;
            };
            let expanded = expand_proxy(proxy);
            if reaches_target(&expanded, target).is_some() {
                holders.push(TailscaleServeRootHolder::Ours);
                continue;
            }
            let holder = match dialed_host_port(&expanded) {
                Some((host, port)) if is_local_loopback_host(&host) => {
                    TailscaleServeRootHolder::Other {
                        loopback: Some(ServeTarget {
                            host: normalize_hostname(&host),
                            port,
                        }),
                    }
                }
                _ => TailscaleServeRootHolder::Other { loopback: None },
            };
            holders.push(holder);
        }
    }
    Some(holders)
}

/// Resolves [`TailscaleServeRootStatus`] from parsed holders and a live listener predicate.
pub fn classify_serve_root_holder(
    holders: &[TailscaleServeRootHolder],
    is_listening: impl Fn(&str, u16) -> bool,
) -> TailscaleServeRootStatus {
    if holders.is_empty() {
        return TailscaleServeRootStatus::None;
    }
    if holders
        .iter()
        .any(|h| matches!(h, TailscaleServeRootHolder::Ours))
    {
        return TailscaleServeRootStatus::Ours;
    }
    let any_live = holders.iter().any(|h| match h {
        TailscaleServeRootHolder::Ours | TailscaleServeRootHolder::Other { loopback: None } => true,
        TailscaleServeRootHolder::Other { loopback: Some(t) } => is_listening(&t.host, t.port),
    });
    if any_live {
        TailscaleServeRootStatus::Other
    } else {
        TailscaleServeRootStatus::Dead
    }
}
