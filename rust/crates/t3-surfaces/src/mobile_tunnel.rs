//! Per-pane terminal theme storage, HTTP-header-only host swapping, and per-`WebView` tunnel
//! isolation (`BUG-26`).
//!
//! Eliminates three shared-state and stream-corruption bugs in mobile native bridges:
//! 1. `T3TerminalView.swift`: single global `t3-terminal-theme-{appearance}.ghostty` file
//!    clobbered across panes -> per-pane content-addressed `TerminalThemeFileStore`.
//! 2. `T3SocksListener.swift`: full-stream `T3HostSwap` mutating `"Host: ..."` inside JSON/HTTP
//!    request bodies and breaking when headers span chunks -> incremental `HttpHeaderHostSwap`
//!    that transitions permanently from `HeaderPhase` to `BodyPassthrough` at `\r\n\r\n`.
//! 3. `T3TunnelWebView.kt` / `T3TunnelWebView.swift`: global `WebView` proxy overrides interfering
//!    across `WebViews` -> `ScopedWebViewTunnelRegistry` isolating endpoints per `webview_id`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use crate::mobile_tunnel_proxy::{HttpHeaderHostSwap, HttpProxyPhase};
pub use crate::mobile_tunnel_theme::{
    TerminalThemeFileStore, ThemeFileEntry, ThemeFileWritePlan, render_ghostty_theme_config,
    sanitize_pane_id,
};

/// Active proxy/tunnel binding isolated to one `webview_id` (`BUG-26`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelEndpointBinding {
    /// Owning preview `WebView` identifier.
    pub webview_id: String,
    /// Target environment identifier.
    pub environment_id: String,
    /// Local loopback proxy host (typically `"127.0.0.1"`).
    pub proxy_host: String,
    /// Local loopback SOCKS listener port.
    pub proxy_port: u16,
    /// Stand-in loopback domain (`"t3lo.test"` or `"localhost"`).
    pub stand_in_host: String,
    /// Optional upstream WebSocket tunnel URL.
    pub tunnel_base_url: Option<String>,
    /// Monotonic binding generation preventing stale async teardown races.
    pub generation: u64,
}

/// Validation and lookup errors for [`ScopedWebViewTunnelRegistry`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TunnelRegistryError {
    /// `webview_id` was empty or whitespace.
    #[error("webview_id must not be empty")]
    EmptyWebViewId,
    /// `environment_id` was empty or whitespace.
    #[error("environment_id must not be empty")]
    EmptyEnvironmentId,
    /// `proxy_host` was empty or whitespace.
    #[error("proxy_host must not be empty")]
    InvalidProxyHost,
    /// `proxy_port` was zero.
    #[error("proxy_port must be non-zero")]
    ZeroProxyPort,
    /// Target `webview_id` is not registered.
    #[error("webview '{0}' is not bound in tunnel registry")]
    WebViewNotFound(String),
}

/// Per-`webview_id` tunnel endpoint/proxy binding registry isolating concurrent preview
/// `WebViews` (`BUG-26`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopedWebViewTunnelRegistry {
    bindings: BTreeMap<String, TunnelEndpointBinding>,
    next_generation: u64,
}

impl ScopedWebViewTunnelRegistry {
    /// Creates an empty per-`WebView` tunnel registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Binds or replaces the tunnel proxy endpoint exclusively for `webview_id`, leaving all other
    /// `WebViews` untouched.
    pub fn bind(
        &mut self,
        webview_id: &str,
        environment_id: &str,
        proxy_host: &str,
        proxy_port: u16,
        stand_in_host: &str,
        tunnel_base_url: Option<&str>,
    ) -> Result<TunnelEndpointBinding, TunnelRegistryError> {
        let wv = non_empty(webview_id).ok_or(TunnelRegistryError::EmptyWebViewId)?;
        let env = non_empty(environment_id).ok_or(TunnelRegistryError::EmptyEnvironmentId)?;
        let host = non_empty(proxy_host).ok_or(TunnelRegistryError::InvalidProxyHost)?;
        if proxy_port == 0 {
            return Err(TunnelRegistryError::ZeroProxyPort);
        }
        let stand_in = non_empty(stand_in_host).unwrap_or("localhost");
        self.next_generation = self.next_generation.saturating_add(1);
        let binding = TunnelEndpointBinding {
            webview_id: wv.to_owned(),
            environment_id: env.to_owned(),
            proxy_host: host.to_owned(),
            proxy_port,
            stand_in_host: stand_in.to_owned(),
            tunnel_base_url: non_empty(tunnel_base_url.unwrap_or_default()).map(str::to_owned),
            generation: self.next_generation,
        };
        self.bindings.insert(wv.to_owned(), binding.clone());
        Ok(binding)
    }

    /// Updates the upstream `tunnel_base_url` solely for `webview_id`.
    pub fn update_tunnel_url(
        &mut self,
        webview_id: &str,
        tunnel_base_url: Option<&str>,
    ) -> Result<TunnelEndpointBinding, TunnelRegistryError> {
        let Some(binding) = self.bindings.get_mut(webview_id) else {
            return Err(TunnelRegistryError::WebViewNotFound(webview_id.to_owned()));
        };
        binding.tunnel_base_url = non_empty(tunnel_base_url.unwrap_or_default()).map(str::to_owned);
        Ok(binding.clone())
    }

    /// Unbinds `webview_id`, leaving every other `WebView` binding intact.
    pub fn unbind(&mut self, webview_id: &str) -> Option<TunnelEndpointBinding> {
        self.bindings.remove(webview_id)
    }

    /// Unbinds `webview_id` only if its active `generation` matches `expected_generation`.
    pub fn unbind_if_generation(
        &mut self,
        webview_id: &str,
        expected_generation: u64,
    ) -> Option<TunnelEndpointBinding> {
        let matches = self
            .bindings
            .get(webview_id)
            .is_some_and(|b| b.generation == expected_generation);
        if matches {
            self.bindings.remove(webview_id)
        } else {
            None
        }
    }

    /// Returns the active binding for `webview_id`, if any.
    pub fn get(&self, webview_id: &str) -> Option<&TunnelEndpointBinding> {
        self.bindings.get(webview_id)
    }

    /// Lists all active `WebView` bindings targeting `environment_id`.
    pub fn bindings_for_environment(&self, environment_id: &str) -> Vec<&TunnelEndpointBinding> {
        self.bindings
            .values()
            .filter(|b| b.environment_id == environment_id)
            .collect()
    }

    /// Total number of currently bound `WebViews`.
    pub fn active_count(&self) -> usize {
        self.bindings.len()
    }

    /// Returns `true` when at least one `WebView` other than `webview_id` still holds a tunnel.
    pub fn has_other_active(&self, webview_id: &str) -> bool {
        self.bindings.keys().any(|id| id != webview_id)
    }
}

fn non_empty(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}
