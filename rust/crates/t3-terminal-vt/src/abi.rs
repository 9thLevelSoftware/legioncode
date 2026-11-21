//! Safe Rust `#[repr(C)]` ABI mirrors for `native/libghostty-vt/include/ghostty/vt.h`.
//!
//! Every C union (`GhosttyStyleColorValue`, `GhosttyTerminalScrollViewportValue`) is represented
//! using aligned byte storage with safe accessors so `#![forbid(unsafe_code)]` holds everywhere
//! while preserving exact C ABI size and alignment across 32-bit (`wasm32`) and 64-bit targets.

use serde::{Deserialize, Serialize};

pub use crate::abi_screen::{
    GhosttyCell, GhosttyCellContentTag, GhosttyCellSemanticContent, GhosttyCellWide,
    GhosttyCursorShape, GhosttyRenderStateCursorVisualStyle, GhosttyRow, GhosttyStyleFlags,
    GhosttyTerminalCursorStyle,
};
pub use crate::abi_terminal::{
    GhosttyMode, GhosttyPointCoordinate, GhosttyPointTag, GhosttyRenderStateColors,
    GhosttyRenderStateDirty, GhosttySizeReportSize, GhosttySizeReportStyle, GhosttyTerminalOptions,
    GhosttyTerminalScreen, GhosttyTerminalScrollViewport, GhosttyTerminalScrollViewportTag,
    GhosttyTerminalScrollViewportValue, GhosttyTerminalScrollbar, GhosttyViewport,
};

/// Result status codes returned by `libghostty-vt` APIs (`GhosttyResult` in `ghostty/vt/types.h`).
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GhosttyResult {
    /// Operation completed successfully (`GHOSTTY_SUCCESS`).
    #[default]
    Success = 0,
    /// Allocation failure (`GHOSTTY_OUT_OF_MEMORY`).
    OutOfMemory = -1,
    /// Invalid input or handle (`GHOSTTY_INVALID_VALUE`).
    InvalidValue = -2,
    /// Caller buffer too small (`GHOSTTY_OUT_OF_SPACE`).
    OutOfSpace = -3,
    /// Requested field has no value (`GHOSTTY_NO_VALUE`).
    NoValue = -4,
}

/// 24-bit RGB color (`GhosttyColorRgb` in `ghostty/vt/color.h`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct GhosttyRgb {
    /// Red channel (`0..=255`).
    pub r: u8,
    /// Green channel (`0..=255`).
    pub g: u8,
    /// Blue channel (`0..=255`).
    pub b: u8,
}

/// Exact C type alias matching `GhosttyColorRgb` in `ghostty/vt/color.h`.
pub type GhosttyColorRgb = GhosttyRgb;

impl GhosttyRgb {
    /// Constructs an RGB color.
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

/// 256-bit mask of palette indices (`GhosttyColorPaletteMask` in `ghostty/vt/color.h`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GhosttyColorPaletteMask {
    /// Four 64-bit words covering palette indices `0..=255`.
    pub bits: [u64; 4],
}

impl GhosttyColorPaletteMask {
    /// Marks palette `index` in the mask.
    pub const fn set(&mut self, index: u8) {
        self.bits[(index >> 6) as usize] |= 1_u64 << (index & 63);
    }

    /// Clears palette `index` in the mask.
    pub const fn unset(&mut self, index: u8) {
        self.bits[(index >> 6) as usize] &= !(1_u64 << (index & 63));
    }

    /// Returns `true` when palette `index` is set.
    pub const fn is_set(&self, index: u8) -> bool {
        (self.bits[(index >> 6) as usize] & (1_u64 << (index & 63))) != 0
    }
}

/// Style color tag (`GhosttyStyleColorTag` in `ghostty/vt/style.h`).
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GhosttyStyleColorTag {
    /// No explicit color (use terminal default).
    #[default]
    None = 0,
    /// 256-color palette index.
    Palette = 1,
    /// Direct 24-bit RGB color.
    Rgb = 2,
}

/// Safe C-ABI mirror for `GhosttyStyleColorValue` (`size = 8, align = 8`).
#[repr(C, align(8))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GhosttyStyleColorValue {
    bytes: [u8; 8],
}

impl GhosttyStyleColorValue {
    /// Encodes a palette index into offset `0`.
    pub const fn from_palette(index: u8) -> Self {
        let mut bytes = [0_u8; 8];
        bytes[0] = index;
        Self { bytes }
    }

    /// Encodes an RGB struct into offsets `0..3`.
    pub const fn from_rgb(rgb: GhosttyRgb) -> Self {
        let mut bytes = [0_u8; 8];
        bytes[0] = rgb.r;
        bytes[1] = rgb.g;
        bytes[2] = rgb.b;
        Self { bytes }
    }

    /// Reads the palette index at offset `0`.
    pub const fn as_palette(self) -> u8 {
        self.bytes[0]
    }

    /// Reads the RGB triple at offsets `0..3`.
    pub const fn as_rgb(self) -> GhosttyRgb {
        GhosttyRgb::new(self.bytes[0], self.bytes[1], self.bytes[2])
    }
}

/// Tagged style color (`GhosttyStyleColor` in `ghostty/vt/style.h`, `size = 16, align = 8`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GhosttyStyleColor {
    /// Discriminant describing which payload variant is active.
    pub tag: GhosttyStyleColorTag,
    /// Aligned 8-byte payload matching C `union GhosttyStyleColorValue`.
    pub value: GhosttyStyleColorValue,
}

impl GhosttyStyleColor {
    /// Unset / default style color.
    pub const fn none() -> Self {
        Self {
            tag: GhosttyStyleColorTag::None,
            value: GhosttyStyleColorValue { bytes: [0; 8] },
        }
    }

    /// Palette-indexed color.
    pub const fn palette(index: u8) -> Self {
        Self {
            tag: GhosttyStyleColorTag::Palette,
            value: GhosttyStyleColorValue::from_palette(index),
        }
    }

    /// Direct 24-bit RGB color.
    pub const fn rgb(rgb: GhosttyRgb) -> Self {
        Self {
            tag: GhosttyStyleColorTag::Rgb,
            value: GhosttyStyleColorValue::from_rgb(rgb),
        }
    }
}

/// Underline style (`GhosttySgrUnderline` in `ghostty/vt/sgr.h`).
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum GhosttySgrUnderline {
    /// No underline.
    #[default]
    None = 0,
    /// Single underline (`SGR 4` / `4:1`).
    Single = 1,
    /// Double underline (`SGR 21` / `4:2`).
    Double = 2,
    /// Curly underline (`SGR 4:3`).
    Curly = 3,
    /// Dotted underline (`SGR 4:4`).
    Dotted = 4,
    /// Dashed underline (`SGR 4:5`).
    Dashed = 5,
}

/// Complete sized cell style (`GhosttyStyle` in `ghostty/vt/style.h`, `size = 72, align = 8`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GhosttyStyle {
    /// `sizeof(GhosttyStyle)` for C ABI versioning.
    pub size: usize,
    /// Foreground style color.
    pub fg_color: GhosttyStyleColor,
    /// Background style color.
    pub bg_color: GhosttyStyleColor,
    /// Underline style color.
    pub underline_color: GhosttyStyleColor,
    /// Bold text attribute.
    pub bold: bool,
    /// Italic text attribute.
    pub italic: bool,
    /// Faint / dim text attribute.
    pub faint: bool,
    /// Blink text attribute.
    pub blink: bool,
    /// Inverse video attribute.
    pub inverse: bool,
    /// Concealed / invisible text attribute.
    pub invisible: bool,
    /// Strikethrough decoration.
    pub strikethrough: bool,
    /// Overline decoration.
    pub overline: bool,
    /// Underline style (`GhosttySgrUnderline as i32`).
    pub underline: i32,
}

impl Default for GhosttyStyle {
    fn default() -> Self {
        Self::new()
    }
}

impl GhosttyStyle {
    /// Creates a default `GhosttyStyle` with `size` initialized to `size_of::<Self>()`.
    pub const fn new() -> Self {
        Self {
            size: std::mem::size_of::<Self>(),
            fg_color: GhosttyStyleColor::none(),
            bg_color: GhosttyStyleColor::none(),
            underline_color: GhosttyStyleColor::none(),
            bold: false,
            italic: false,
            faint: false,
            blink: false,
            inverse: false,
            invisible: false,
            strikethrough: false,
            overline: false,
            underline: GhosttySgrUnderline::None as i32,
        }
    }
}
