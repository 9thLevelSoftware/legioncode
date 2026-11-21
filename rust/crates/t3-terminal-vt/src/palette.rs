//! Sparse 256-color ANSI palette resolution (`BUG-25`).
//!
//! `AnsiPalette256` stores `[Option<Rgba>; 256]` override slots:
//! - Applying an empty theme palette resets all 256 slots to `None` so prior theme colors never
//!   linger.
//! - Sparse theme overrides leave unspecified slots as `None` (never filling gaps with
//!   `foreground_color`) and resolve fallback entries via the canonical xterm-256 RGB table.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::PALETTE_LEN;
use crate::abi::GhosttyRgb;
pub use crate::theme_config::{TerminalThemeConfig, parse_color, parse_palette_entry};
pub use crate::xterm256::{XTERM_256_PALETTE, xterm_256_rgb};

/// 32-bit `RGBA` color with `ARGB` (`0xAARRGGBB`) wire conversion for `"T3VT"` snapshots.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Rgba {
    /// Red channel (`0..=255`).
    pub r: u8,
    /// Green channel (`0..=255`).
    pub g: u8,
    /// Blue channel (`0..=255`).
    pub b: u8,
    /// Alpha channel (`0..=255`, `255` = opaque).
    pub a: u8,
}

impl Rgba {
    /// Opaque black (`#000000`).
    pub const BLACK: Self = Self::rgb(0, 0, 0);
    /// Opaque white (`#FFFFFF`).
    pub const WHITE: Self = Self::rgb(255, 255, 255);

    /// Creates a color with explicit RGBA channels.
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// Creates an opaque color (`a = 255`).
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// Converts from a [`GhosttyRgb`] assuming full opacity.
    pub const fn from_rgb(rgb: GhosttyRgb) -> Self {
        Self::rgb(rgb.r, rgb.g, rgb.b)
    }

    /// Drops alpha and returns a [`GhosttyRgb`].
    pub const fn to_rgb(self) -> GhosttyRgb {
        GhosttyRgb::new(self.r, self.g, self.b)
    }

    /// Decodes a packed `0xAARRGGBB` word used by Android `Color` and `"T3VT"` snapshots.
    pub const fn from_argb_u32(argb: u32) -> Self {
        Self {
            a: ((argb >> 24) & 0xFF) as u8,
            r: ((argb >> 16) & 0xFF) as u8,
            g: ((argb >> 8) & 0xFF) as u8,
            b: (argb & 0xFF) as u8,
        }
    }

    /// Encodes a packed `0xAARRGGBB` word matching `ArgbFromRgb` in `t3_terminal_jni.cpp`.
    pub const fn to_argb_u32(self) -> u32 {
        ((self.a as u32) << 24) | ((self.r as u32) << 16) | ((self.g as u32) << 8) | (self.b as u32)
    }

    /// Linearly blends `self` over `background` using Ghostty's integer `foreground_weight / 255`
    /// formula (`t3_terminal_jni.cpp:93-104`).
    #[must_use]
    pub const fn blend_over(self, background: Self, foreground_weight: u8) -> Self {
        let fw = foreground_weight as u16;
        let bw = 255_u16 - fw;
        Self {
            r: blend_channel(self.r, background.r, fw, bw),
            g: blend_channel(self.g, background.g, fw, bw),
            b: blend_channel(self.b, background.b, fw, bw),
            a: self.a,
        }
    }
}

const fn blend_channel(front: u8, back: u8, fw: u16, bw: u16) -> u8 {
    let mixed = (front as u16 * fw + back as u16 * bw) / 255;
    if mixed > 255 {
        255
    } else {
        mixed.to_le_bytes()[0]
    }
}

impl From<GhosttyRgb> for Rgba {
    fn from(value: GhosttyRgb) -> Self {
        Self::from_rgb(value)
    }
}

impl From<Rgba> for GhosttyRgb {
    fn from(value: Rgba) -> Self {
        value.to_rgb()
    }
}

/// Sparse 256-entry ANSI color palette backed by `[Option<Rgba>; 256]` (`BUG-25`).
///
/// Unset slots remain `None` and resolve dynamically via [`xterm_256_rgb`], never inheriting
/// stale colors from an earlier theme or falling back to a caller's `foreground_color`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnsiPalette256 {
    overrides: [Option<Rgba>; PALETTE_LEN],
}

impl Default for AnsiPalette256 {
    fn default() -> Self {
        Self::new()
    }
}

impl AnsiPalette256 {
    /// Creates a palette with all `256` override slots initialized to `None`.
    pub const fn new() -> Self {
        Self {
            overrides: [None; PALETTE_LEN],
        }
    }

    /// Resets every override slot (`0..=255`) to `None`.
    pub fn clear(&mut self) {
        self.overrides = [None; PALETTE_LEN];
    }

    /// Replaces all overrides atomically with `overrides` (`BUG-25`).
    ///
    /// Passing an empty slice `&[]` resets all `256` slots to `None` so stale colors from a
    /// previous theme never survive. Any index omitted from a sparse slice stays `None`.
    pub fn apply_theme_overrides(&mut self, overrides: &[(u8, Rgba)]) {
        self.overrides = [None; PALETTE_LEN];
        for &(index, color) in overrides {
            self.overrides[usize::from(index)] = Some(color);
        }
    }

    /// Replaces overrides from a dense optional slice (`0..overrides.len().min(256)`), resetting
    /// all `256` slots first so trailing or omitted indices become `None`.
    pub fn apply_dense_overrides(&mut self, overrides: &[Option<Rgba>]) {
        self.overrides = [None; PALETTE_LEN];
        for (slot, &value) in self.overrides.iter_mut().zip(overrides.iter()) {
            *slot = value;
        }
    }

    /// Applies palette entries from a Ghostty theme config string, resetting all `256` slots
    /// first (`BUG-25`).
    pub fn apply_theme_config(&mut self, config: &str) {
        let parsed = TerminalThemeConfig::parse(config);
        self.overrides = parsed.palette.overrides;
    }

    /// Sets or clears a single override slot (`0..=255`).
    pub fn set_override(&mut self, index: u8, color: Option<Rgba>) {
        self.overrides[usize::from(index)] = color;
    }

    /// Returns the raw override at `index`, or `None` if unspecified.
    pub const fn override_at(&self, index: u8) -> Option<Rgba> {
        self.overrides[index as usize]
    }

    /// Borrows the underlying `[Option<Rgba>; 256]` sparse slot array.
    pub const fn overrides(&self) -> &[Option<Rgba>; PALETTE_LEN] {
        &self.overrides
    }

    /// Collects all active `(index, color)` override pairs in ascending index order.
    pub fn override_pairs(&self) -> Vec<(u8, Rgba)> {
        let mut out = Vec::new();
        for (idx, slot) in self.overrides.iter().enumerate() {
            if let Some(color) = slot
                && let Ok(index) = u8::try_from(idx)
            {
                out.push((index, *color));
            }
        }
        out
    }

    /// Resolves `index` to its theme override or the canonical xterm-256 fallback color.
    pub const fn resolve(&self, index: u8) -> Rgba {
        match self.overrides[index as usize] {
            Some(color) => color,
            None => Rgba::from_rgb(xterm_256_rgb(index)),
        }
    }

    /// Resolves `index` to an opaque [`GhosttyRgb`].
    pub const fn resolve_rgb(&self, index: u8) -> GhosttyRgb {
        self.resolve(index).to_rgb()
    }

    /// Materializes all `256` resolved [`Rgba`] entries.
    pub fn resolve_all(&self) -> [Rgba; PALETTE_LEN] {
        std::array::from_fn(|idx| {
            let index = u8::try_from(idx).unwrap_or(u8::MAX);
            self.resolve(index)
        })
    }

    /// Materializes all `256` resolved [`GhosttyRgb`] entries for `GhosttyRenderStateColors`.
    pub fn resolve_all_rgb(&self) -> [GhosttyRgb; PALETTE_LEN] {
        std::array::from_fn(|idx| {
            let index = u8::try_from(idx).unwrap_or(u8::MAX);
            self.resolve_rgb(index)
        })
    }
}

impl Serialize for AnsiPalette256 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.override_pairs().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for AnsiPalette256 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let pairs = Vec::<(u8, Rgba)>::deserialize(deserializer)?;
        let mut palette = Self::new();
        palette.apply_theme_overrides(&pairs);
        Ok(palette)
    }
}
