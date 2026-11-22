//! `BUG-25` palette and `"LCVT"` snapshot request/response schemas for [`crate::core`].

use legion_terminal_vt::abi::GhosttyCursorShape;
use legion_terminal_vt::palette::{Rgba, parse_color};
use serde::{Deserialize, Serialize};

use crate::core_types::MobileFfiError;

/// Sparse ANSI 256-color palette override entry accepted in JSON requests (`BUG-25`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PaletteOverrideInput {
    /// Tuple `[index, { r, g, b, a }]` matching `AnsiPalette256` serialization.
    Pair(u8, Rgba),
    /// Named object `{ "index": 1, "color": "#ff2e3f" }`.
    Hex {
        /// Palette index (`0..=255`).
        index: u8,
        /// CSS/Ghostty/X11 color string (`#RGB`, `#RRGGBB`, `#RRGGBBAA`, `rgb:R/G/B`).
        color: String,
    },
    /// Named RGBA object `{ "index": 1, "r": 255, "g": 0, "b": 0, "a": 255 }`.
    Rgba {
        /// Palette index (`0..=255`).
        index: u8,
        /// Red channel (`0..=255`).
        r: u8,
        /// Green channel (`0..=255`).
        g: u8,
        /// Blue channel (`0..=255`).
        b: u8,
        /// Alpha channel (`0..=255`, defaults to `255`).
        #[serde(default = "default_opaque_alpha")]
        a: u8,
    },
}

const fn default_opaque_alpha() -> u8 {
    255
}

impl PaletteOverrideInput {
    pub(crate) fn into_pair(self) -> Result<(u8, Rgba), MobileFfiError> {
        match self {
            Self::Pair(index, color) => Ok((index, color)),
            Self::Hex { index, color } => Ok((index, parse_color(&color)?)),
            Self::Rgba { index, r, g, b, a } => Ok((index, Rgba::new(r, g, b, a))),
        }
    }
}

/// Request payload for [`crate::core::palette_resolve_json`] (`BUG-25`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaletteResolveRequest {
    /// Optional initial overrides seeded before applying the next theme (`BUG-25` reset check).
    #[serde(default)]
    pub prior_overrides: Option<Vec<PaletteOverrideInput>>,
    /// Optional Ghostty-style theme config text (`foreground = ...`, `palette = idx=color`).
    #[serde(default)]
    pub theme_config: Option<String>,
    /// Optional explicit sparse overrides applied atomically (empty slice resets all 256 slots).
    #[serde(default)]
    pub overrides: Option<Vec<PaletteOverrideInput>>,
    /// Specific indices (`0..=255`) to resolve; defaults to all `0..=255` when omitted.
    #[serde(default)]
    pub indices: Option<Vec<u8>>,
}

/// Resolved RGBA color payload returned in palette and VT snapshot JSON responses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RgbaOutput {
    /// Red channel (`0..=255`).
    pub r: u8,
    /// Green channel (`0..=255`).
    pub g: u8,
    /// Blue channel (`0..=255`).
    pub b: u8,
    /// Alpha channel (`0..=255`).
    pub a: u8,
    /// Packed `0xAARRGGBB` word.
    pub argb: u32,
    /// Lowercase `#rrggbb` (when `a == 255`) or `#rrggbbaa` string.
    pub hex: String,
}

impl RgbaOutput {
    pub(crate) fn from_rgba(color: Rgba) -> Self {
        let hex = if color.a == 255 {
            format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b)
        } else {
            format!(
                "#{:02x}{:02x}{:02x}{:02x}",
                color.r, color.g, color.b, color.a
            )
        };
        Self {
            r: color.r,
            g: color.g,
            b: color.b,
            a: color.a,
            argb: color.to_argb_u32(),
            hex,
        }
    }
}

/// Single resolved ANSI 256-color entry (`BUG-25`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaletteEntryOutput {
    /// Palette index (`0..=255`).
    pub index: u8,
    /// `true` when explicitly set by theme overrides; `false` when resolved via xterm-256 table.
    pub is_override: bool,
    /// Resolved RGBA color details.
    #[serde(flatten)]
    pub color: RgbaOutput,
}

/// Response payload returned by [`crate::core::palette_resolve_json`] (`BUG-25`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaletteResolveResponse {
    /// Active sparse overrides in ascending index order.
    pub overrides: Vec<PaletteEntryOutput>,
    /// Number of active sparse override slots (`0..=256`).
    pub override_count: usize,
    /// Resolved palette entries for `request.indices` (or all `256` entries when omitted).
    pub resolved: Vec<PaletteEntryOutput>,
    /// Parsed default foreground color from `themeConfig`, if specified.
    pub foreground: Option<RgbaOutput>,
    /// Parsed default background color from `themeConfig`, if specified.
    pub background: Option<RgbaOutput>,
    /// Parsed cursor color from `themeConfig`, if specified.
    pub cursor_color: Option<RgbaOutput>,
    /// Parsed cursor text color from `themeConfig`, if specified.
    pub cursor_text: Option<RgbaOutput>,
}

/// Request payload for [`crate::core::vt_render_snapshot`].
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VtRenderSnapshotRequest {
    /// Viewport width in columns (`1..=1024`).
    pub cols: u16,
    /// Viewport height in rows (`1..=512`).
    pub rows: u16,
    /// Raw ANSI/VT UTF-8 text fed through [`legion_terminal_vt::VtParser`].
    #[serde(default)]
    pub ansi_text: Option<String>,
    /// Raw ANSI/VT byte stream fed before `ansiText` through [`legion_terminal_vt::VtParser`].
    #[serde(default)]
    pub bytes: Option<Vec<u8>>,
    /// Optional Ghostty theme configuration applied to the terminal grid.
    #[serde(default)]
    pub theme_config: Option<String>,
    /// Optional explicit sparse palette overrides applied atomically.
    #[serde(default)]
    pub palette_overrides: Option<Vec<PaletteOverrideInput>>,
    /// `"LCVT"` wire version (`2` by default; `1` for legacy Android `TerminalFrame.kt` interop).
    #[serde(default)]
    pub version: Option<u16>,
}

/// Decoded snapshot cell payload returned by [`crate::core::vt_decode_snapshot_json`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VtDecodedCellOutput {
    /// Resolved cell foreground color.
    pub foreground: RgbaOutput,
    /// Resolved cell background color.
    pub background: RgbaOutput,
    /// Raw `GhosttyStyleFlags` 16-bit attribute mask.
    pub flags: u16,
    /// UTF-8 grapheme cluster text (`""` for blank cells).
    pub text: String,
}

/// Decoded `"LCVT"` snapshot summary returned by [`crate::core::vt_decode_snapshot_json`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VtDecodedSnapshotResponse {
    /// Viewport width in columns.
    pub cols: u16,
    /// Viewport height in rows.
    pub rows: u16,
    /// Cursor column (`0xFFFF` when outside viewport).
    pub cursor_x: u16,
    /// Cursor row (`0xFFFF` when outside viewport).
    pub cursor_y: u16,
    /// Cursor visibility flag.
    pub cursor_visible: bool,
    /// Visual cursor shape.
    pub cursor_style: GhosttyCursorShape,
    /// Cursor blink flag.
    pub cursor_blinking: bool,
    /// Alternate screen active flag.
    pub alternate_screen: bool,
    /// Terminal default foreground color.
    pub foreground: RgbaOutput,
    /// Terminal default background color.
    pub background: RgbaOutput,
    /// Terminal default cursor color.
    pub cursor_color: RgbaOutput,
    /// Active sparse 256-color palette overrides.
    pub palette_overrides: Vec<PaletteEntryOutput>,
    /// Row-major viewport cell entries (`cols * rows`).
    pub cells: Vec<VtDecodedCellOutput>,
    /// Plain row strings joined per row with trailing ASCII whitespace trimmed.
    pub viewport_lines: Vec<String>,
}
