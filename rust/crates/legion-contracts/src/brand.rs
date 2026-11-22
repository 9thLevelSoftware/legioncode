//! Brand configuration and canonical `LegionCode` identity helpers.

use serde::{Deserialize, Serialize};

/// Product branding constants controlling CLI names, environment variables, pipes, headers, and policies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrandConfig {
    /// Human-facing product name.
    pub product_name: &'static str,
    /// Primary product domain.
    pub org_domain: &'static str,
    /// CLI executable name.
    pub cli_bin_name: &'static str,
    /// Desktop executable name.
    pub desktop_bin_name: &'static str,
    /// Environment variable prefix (including trailing underscore).
    pub primary_env_prefix: &'static str,
    /// Home directory folder under `$HOME` / `%USERPROFILE%`.
    pub home_dir_name: &'static str,
    /// Desktop deep-link URL scheme without `://`.
    pub url_scheme: &'static str,
    /// Desktop single-instance IPC pipe / socket prefix.
    pub ipc_pipe_prefix: &'static str,
    /// HTTP header prefix for forwarded and tunnel headers.
    pub http_header_prefix: &'static str,
    /// MCP tool name prefix (`legion_`).
    pub mcp_tool_prefix: &'static str,
    /// MCP provider namespace prefix (`mcp__legion_code__`).
    pub mcp_namespace: &'static str,
    /// Linux systemd user unit filename.
    pub systemd_unit_name: &'static str,
    /// macOS launchd job label.
    pub launchd_label: &'static str,
    /// Windows Task Scheduler base name.
    pub windows_task_name: &'static str,
    /// Windows Group Policy registry key path.
    pub windows_policy_key: &'static str,
    /// macOS managed preferences / mobileconfig domain.
    pub macos_policy_domain: &'static str,
}

/// Canonical `LegionCode` brand configuration.
pub const LEGION_CODE: BrandConfig = BrandConfig {
    product_name: "LegionCode",
    org_domain: "legion.codes",
    cli_bin_name: "legioncode",
    desktop_bin_name: "legioncode-desktop",
    primary_env_prefix: "LEGIONCODE_",
    home_dir_name: ".legioncode",
    url_scheme: "legioncode",
    ipc_pipe_prefix: "legioncode",
    http_header_prefix: "x-legioncode-",
    mcp_tool_prefix: "legion_",
    mcp_namespace: "mcp__legion_code__",
    systemd_unit_name: "legioncode.service",
    launchd_label: "dev.legioncode.service",
    windows_task_name: "legioncode",
    windows_policy_key: r"SOFTWARE\Policies\LegionCode\LegionCode",
    macos_policy_domain: "dev.legioncode",
};

/// Active compile-time brand configuration.
pub const ACTIVE_BRAND: BrandConfig = LEGION_CODE;

impl BrandConfig {
    /// Returns the canonical environment variable name (`primary_env_prefix + suffix`).
    pub fn env_var_candidates(&self, suffix: &str) -> Vec<String> {
        vec![format!("{}{suffix}", self.primary_env_prefix)]
    }

    /// Reads the non-empty environment variable matching [`Self::env_var_candidates`] using `lookup`.
    pub fn read_env_from(
        &self,
        suffix: &str,
        mut lookup: impl FnMut(&str) -> Option<String>,
    ) -> Option<String> {
        self.env_var_candidates(suffix)
            .into_iter()
            .find_map(|key| lookup(&key).filter(|val| !val.trim().is_empty()))
    }

    /// Reads the non-empty process environment variable matching [`Self::env_var_candidates`].
    pub fn read_env(&self, suffix: &str) -> Option<String> {
        self.read_env_from(suffix, |key| std::env::var(key).ok())
    }

    /// Strips known `LegionCode` MCP namespaces and tool prefixes (`mcp__legion_code__`, `mcp__legion-code__`, `legion_`).
    pub fn canonical_mcp_tool_suffix<'a>(&self, tool_name: &'a str) -> &'a str {
        let mut s = tool_name;
        for ns in [
            self.mcp_namespace,
            "mcp__legion_code__",
            "mcp__legion-code__",
        ] {
            if let Some(rest) = s.strip_prefix(ns) {
                s = rest;
                break;
            }
        }
        for prefix in [self.mcp_tool_prefix, "legion_"] {
            if let Some(rest) = s.strip_prefix(prefix) {
                s = rest;
                break;
            }
        }
        s
    }

    /// Returns the canonical HTTP header name (`http_header_prefix + suffix`).
    pub fn tunnel_header_candidates(&self, suffix: &str) -> Vec<String> {
        vec![format!("{}{suffix}", self.http_header_prefix)]
    }

    /// Returns the Windows named-pipe prefix (`\\.\pipe\<ipc_pipe_prefix>-app-`).
    pub fn desktop_pipe_prefix(&self) -> String {
        format!(r"\\.\pipe\{}-app-", self.ipc_pipe_prefix)
    }
}

/// Returns ordered environment variable candidates for [`ACTIVE_BRAND`].
pub fn env_var_candidates(suffix: &str) -> Vec<String> {
    ACTIVE_BRAND.env_var_candidates(suffix)
}

/// Reads the first non-empty candidate variable via `lookup` for [`ACTIVE_BRAND`].
pub fn read_env_from(suffix: &str, lookup: impl FnMut(&str) -> Option<String>) -> Option<String> {
    ACTIVE_BRAND.read_env_from(suffix, lookup)
}

/// Canonicalizes an MCP tool name for [`ACTIVE_BRAND`].
pub fn canonical_mcp_tool_suffix(tool_name: &str) -> &str {
    ACTIVE_BRAND.canonical_mcp_tool_suffix(tool_name)
}

/// Returns tunnel HTTP header candidates for [`ACTIVE_BRAND`].
pub fn tunnel_header_candidates(suffix: &str) -> Vec<String> {
    ACTIVE_BRAND.tunnel_header_candidates(suffix)
}

/// Returns the Windows desktop pipe prefix for [`ACTIVE_BRAND`].
pub fn desktop_pipe_prefix() -> String {
    ACTIVE_BRAND.desktop_pipe_prefix()
}
