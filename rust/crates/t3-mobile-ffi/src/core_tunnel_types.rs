//! `BUG-26` HTTP proxy host-swap and per-pane theme plan schemas for [`crate::core`].

use serde::{Deserialize, Serialize};
use t3_surfaces::mobile_tunnel::{HttpProxyPhase, ThemeFileEntry};

/// One incoming HTTP proxy chunk (`UTF-8` string or raw bytes) (`BUG-26`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ChunkInput {
    /// UTF-8 text chunk.
    Text(String),
    /// Raw byte array chunk.
    Bytes(Vec<u8>),
}

impl ChunkInput {
    pub(crate) fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Text(text) => text.as_bytes(),
            Self::Bytes(bytes) => bytes.as_slice(),
        }
    }
}

/// Request payload for [`crate::core::host_swap_chunks_json`] (`BUG-26`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostSwapChunksRequest {
    /// Source hostname matched case-insensitively inside HTTP `Host:` headers.
    pub from_host: String,
    /// Replacement hostname written into matching `Host:` headers.
    pub to_host: String,
    /// Ordered incoming stream chunks (headers can split across chunks; body is passthrough).
    #[serde(default)]
    pub chunks: Vec<ChunkInput>,
    /// Whether to call [`t3_surfaces::mobile_tunnel::HttpHeaderHostSwap::finish`] after all chunks.
    #[serde(default = "default_true")]
    pub finish: bool,
}

const fn default_true() -> bool {
    true
}

/// Response payload returned by [`crate::core::host_swap_chunks_json`] (`BUG-26`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostSwapChunksResponse {
    /// Proxy state after processing all chunks (`HeaderPhase` or `BodyPassthrough`).
    pub phase: HttpProxyPhase,
    /// 0-based index of the chunk that crossed `\r\n\r\n` into `BodyPassthrough`, if reached.
    pub transitioned_at_chunk: Option<usize>,
    /// Lossy UTF-8 outputs emitted for each pushed chunk (plus final flush when non-empty).
    pub output_chunks: Vec<String>,
    /// Complete reassembled UTF-8 stream output.
    pub output: String,
    /// Exact reassembled output bytes.
    pub output_bytes: Vec<u8>,
}

/// Action kind inside [`ThemePlanOperation`] (`BUG-26`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ThemePlanAction {
    /// Upsert theme configuration for `paneId`.
    #[default]
    Upsert,
    /// Remove `paneId` from the theme file store.
    Remove,
}

/// One seeded or stepped terminal theme config operation (`BUG-26`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemePlanOperation {
    /// Operation action (`upsert` or `remove`).
    #[serde(default)]
    pub action: ThemePlanAction,
    /// Terminal pane identifier (`t3code-ghostty-{sanitized}-{hash12}.conf`).
    pub pane_id: String,
    /// Ghostty theme config body passed to `render_ghostty_theme_config`.
    #[serde(default)]
    pub theme_config: Option<String>,
    /// Terminal font size (defaults to `14.0` when rendering from `themeConfig`).
    #[serde(default)]
    pub font_size: Option<f64>,
    /// Optional color scheme marker (`"dark"`, `"light"`).
    #[serde(default)]
    pub appearance: Option<String>,
    /// Pre-rendered config contents used verbatim when specified instead of `themeConfig`.
    #[serde(default)]
    pub contents: Option<String>,
}

/// Request payload for [`crate::core::theme_plan_json`] (`BUG-26`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemePlanRequest {
    /// Optional single-pane identifier to upsert after `priorEntries` and `operations`.
    #[serde(default)]
    pub pane_id: Option<String>,
    /// Ghostty theme config body for top-level `paneId`.
    #[serde(default)]
    pub theme_config: Option<String>,
    /// Font size for top-level `paneId`.
    #[serde(default)]
    pub font_size: Option<f64>,
    /// Optional appearance (`"dark"` / `"light"`) for top-level `paneId`.
    #[serde(default)]
    pub appearance: Option<String>,
    /// Pre-rendered contents for top-level `paneId`.
    #[serde(default)]
    pub contents: Option<String>,
    /// Existing pane entries seeded before running `operations` or top-level `paneId`.
    #[serde(default)]
    pub prior_entries: Vec<ThemePlanOperation>,
    /// Ordered sequence of `upsert` / `remove` theme operations.
    #[serde(default)]
    pub operations: Vec<ThemePlanOperation>,
}

/// Per-step result emitted inside [`ThemePlanResponse`] (`BUG-26`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemePlanStepOutput {
    /// Action executed (`upsert` or `remove`).
    pub action: ThemePlanAction,
    /// Original pane identifier.
    pub pane_id: String,
    /// Sanitized single-component pane identifier.
    pub sanitized_pane_id: String,
    /// Content-addressed filename for upserts or removed entry.
    pub filename: Option<String>,
    /// Rendered Ghostty config contents for upserts.
    pub contents: Option<String>,
    /// Whether the upsert produced new or rotated file contents.
    pub changed: Option<bool>,
    /// Previous filename owned by the same `paneId` to garbage-collect, if rotated.
    pub obsolete_filename: Option<String>,
    /// `true` when `remove` deleted an existing pane entry.
    pub removed: bool,
}

/// Response payload returned by [`crate::core::theme_plan_json`] (`BUG-26`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemePlanResponse {
    /// Active pane count after applying all operations.
    pub pane_count: usize,
    /// Sanitized pane identifier from the final operation, if any.
    pub sanitized_pane_id: Option<String>,
    /// Filename from the final operation, if any.
    pub filename: Option<String>,
    /// Config payload from the final upsert, if any.
    pub contents: Option<String>,
    /// `changed` flag from the final upsert, if any.
    pub changed: Option<bool>,
    /// `obsoleteFilename` from the final upsert, if any.
    pub obsolete_filename: Option<String>,
    /// Step-by-step plans for `operations` and top-level `paneId`.
    pub results: Vec<ThemePlanStepOutput>,
    /// Active entries remaining in [`t3_surfaces::mobile_tunnel::TerminalThemeFileStore`].
    pub entries: Vec<ThemeFileEntry>,
}
