//! Screen cell, row, cursor shape, and style flag ABI mirrors (`ghostty/vt/screen.h` & `style.h`).

use std::ops::{BitAnd, BitAndAssign, BitOr, BitOrAssign};

use serde::{Deserialize, Serialize};

use crate::abi::GhosttyStyle;

/// Compact 16-bit cell style bitflags matching the `"T3VT"` wire format (`t3_terminal_jni.cpp`).
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct GhosttyStyleFlags(pub u16);

impl GhosttyStyleFlags {
    /// No active style flags.
    pub const NONE: Self = Self(0);
    /// Bold (`1 << 0`).
    pub const BOLD: Self = Self(1 << 0);
    /// Italic (`1 << 1`).
    pub const ITALIC: Self = Self(1 << 1);
    /// Faint (`1 << 2`).
    pub const FAINT: Self = Self(1 << 2);
    /// Inverse video (`1 << 3`).
    pub const INVERSE: Self = Self(1 << 3);
    /// Invisible text (`1 << 4`).
    pub const INVISIBLE: Self = Self(1 << 4);
    /// Strikethrough (`1 << 5`).
    pub const STRIKETHROUGH: Self = Self(1 << 5);
    /// Overline (`1 << 6`).
    pub const OVERLINE: Self = Self(1 << 6);
    /// Underline (`1 << 7`).
    pub const UNDERLINE: Self = Self(1 << 7);
    /// Selected cell highlight (`1 << 8`).
    pub const SELECTED: Self = Self(1 << 8);
    /// Blink (`1 << 9`).
    pub const BLINK: Self = Self(1 << 9);

    /// Returns the raw 16-bit mask.
    pub const fn bits(self) -> u16 {
        self.0
    }

    /// Returns `true` if all bits in `other` are present in `self`.
    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    /// Sets the bits from `other`.
    pub const fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }

    /// Clears the bits from `other`.
    pub const fn remove(&mut self, other: Self) {
        self.0 &= !other.0;
    }

    /// Derives cell flags from a full [`GhosttyStyle`].
    pub const fn from_style(style: &GhosttyStyle, selected: bool) -> Self {
        let mut bits = 0_u16;
        if style.bold {
            bits |= Self::BOLD.0;
        }
        if style.italic {
            bits |= Self::ITALIC.0;
        }
        if style.faint {
            bits |= Self::FAINT.0;
        }
        if style.inverse {
            bits |= Self::INVERSE.0;
        }
        if style.invisible {
            bits |= Self::INVISIBLE.0;
        }
        if style.strikethrough {
            bits |= Self::STRIKETHROUGH.0;
        }
        if style.overline {
            bits |= Self::OVERLINE.0;
        }
        if style.underline != 0 {
            bits |= Self::UNDERLINE.0;
        }
        if selected {
            bits |= Self::SELECTED.0;
        }
        if style.blink {
            bits |= Self::BLINK.0;
        }
        Self(bits)
    }
}

impl BitOr for GhosttyStyleFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for GhosttyStyleFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl BitAnd for GhosttyStyleFlags {
    type Output = Self;

    fn bitand(self, rhs: Self) -> Self::Output {
        Self(self.0 & rhs.0)
    }
}

impl BitAndAssign for GhosttyStyleFlags {
    fn bitand_assign(&mut self, rhs: Self) {
        self.0 &= rhs.0;
    }
}

/// Visual cursor shape (`GhosttyTerminalCursorStyle` / `GhosttyRenderStateCursorVisualStyle`).
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum GhosttyCursorShape {
    /// Vertical bar cursor (`DECSCUSR 5, 6`).
    Bar = 0,
    /// Solid block cursor (`DECSCUSR 1, 2`).
    #[default]
    Block = 1,
    /// Horizontal underline cursor (`DECSCUSR 3, 4`).
    Underline = 2,
    /// Hollow block cursor.
    BlockHollow = 3,
}

/// Exact C type alias matching `GhosttyTerminalCursorStyle` in `ghostty/vt/terminal.h`.
pub type GhosttyTerminalCursorStyle = GhosttyCursorShape;

/// Exact C type alias matching `GhosttyRenderStateCursorVisualStyle` in `ghostty/vt/render.h`.
pub type GhosttyRenderStateCursorVisualStyle = GhosttyCursorShape;

impl GhosttyCursorShape {
    /// Converts from a wire `u8` cursor style byte (`0..=3`).
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Bar),
            1 => Some(Self::Block),
            2 => Some(Self::Underline),
            3 => Some(Self::BlockHollow),
            _ => None,
        }
    }

    /// Converts to the `"T3VT"` wire byte (`0..=3`).
    pub const fn to_u8(self) -> u8 {
        self as u8
    }
}

/// Cell content discriminator (`GhosttyCellContentTag` in `ghostty/vt/screen.h`).
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GhosttyCellContentTag {
    /// Single Unicode scalar value (`0` for empty).
    #[default]
    Codepoint = 0,
    /// Base codepoint of a multi-codepoint grapheme cluster.
    CodepointGrapheme = 1,
    /// Background-only cell using a palette index.
    BgColorPalette = 2,
    /// Background-only cell using direct RGB.
    BgColorRgb = 3,
}

/// Cell width class (`GhosttyCellWide` in `ghostty/vt/screen.h`).
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum GhosttyCellWide {
    /// Single-column glyph.
    #[default]
    Narrow = 0,
    /// Leading cell of a double-width glyph.
    Wide = 1,
    /// Trailing spacer cell after a wide glyph.
    SpacerTail = 2,
    /// Leading spacer at the end of a soft-wrapped line before a wide glyph.
    SpacerHead = 3,
}

/// Semantic prompt classification (`GhosttyCellSemanticContent` in `ghostty/vt/screen.h`).
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum GhosttyCellSemanticContent {
    /// Regular command output.
    #[default]
    Output = 0,
    /// User command input.
    Input = 1,
    /// Shell prompt text (`OSC 133`).
    Prompt = 2,
}

/// Packed 64-bit terminal cell (`typedef uint64_t GhosttyCell` in `ghostty/vt/screen.h`).
///
/// Mirrors the exact bit layout probed from `libghostty-vt`:
/// - bits `0..=1`: [`GhosttyCellContentTag`]
/// - bits `2..=25`: codepoint (`2..=22`), palette index (`2..=9`), or `RGB` bytes
/// - bits `26..=41`: `style_id` (`u16`)
/// - bits `42..=43`: [`GhosttyCellWide`]
/// - bit `44`: `protected`
/// - bit `45`: `hyperlink`
/// - bits `46..=47`: [`GhosttyCellSemanticContent`]
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GhosttyCell(pub u64);

impl GhosttyCell {
    /// Constructs a cell holding a codepoint (or empty when `codepoint` is `0`).
    pub const fn from_codepoint(
        codepoint: u32,
        style_id: u16,
        wide: GhosttyCellWide,
        has_hyperlink: bool,
        grapheme: bool,
    ) -> Self {
        let tag = if grapheme {
            GhosttyCellContentTag::CodepointGrapheme as u64
        } else {
            GhosttyCellContentTag::Codepoint as u64
        };
        let cp = (codepoint as u64 & 0x1F_FFFF) << 2;
        let sid = (style_id as u64) << 26;
        let w = ((wide as u64) & 0b11) << 42;
        let hl = (has_hyperlink as u64) << 45;
        Self(tag | cp | sid | w | hl)
    }

    /// Returns the content tag in bits `0..=1`.
    pub const fn content_tag(self) -> GhosttyCellContentTag {
        match self.0 & 0b11 {
            1 => GhosttyCellContentTag::CodepointGrapheme,
            2 => GhosttyCellContentTag::BgColorPalette,
            3 => GhosttyCellContentTag::BgColorRgb,
            _ => GhosttyCellContentTag::Codepoint,
        }
    }

    /// Returns the primary Unicode scalar value (`0` for empty or background-only cells).
    pub const fn codepoint(self) -> u32 {
        match self.content_tag() {
            GhosttyCellContentTag::Codepoint | GhosttyCellContentTag::CodepointGrapheme => {
                ((self.0 >> 2) & 0x1F_FFFF) as u32
            }
            GhosttyCellContentTag::BgColorPalette | GhosttyCellContentTag::BgColorRgb => 0,
        }
    }

    /// Returns whether the cell holds non-empty text.
    pub const fn has_text(self) -> bool {
        self.codepoint() != 0
    }

    /// Returns the 16-bit style table identifier (`0` for default style).
    pub const fn style_id(self) -> u16 {
        ((self.0 >> 26) & 0xFFFF) as u16
    }

    /// Returns whether the cell uses a non-default `style_id`.
    pub const fn has_styling(self) -> bool {
        self.style_id() != 0
    }

    /// Returns the width classification (`GhosttyCellWide`).
    pub const fn wide(self) -> GhosttyCellWide {
        match (self.0 >> 42) & 0b11 {
            1 => GhosttyCellWide::Wide,
            2 => GhosttyCellWide::SpacerTail,
            3 => GhosttyCellWide::SpacerHead,
            _ => GhosttyCellWide::Narrow,
        }
    }

    /// Returns whether the cell is protected (`DECSCA`).
    pub const fn is_protected(self) -> bool {
        ((self.0 >> 44) & 1) != 0
    }

    /// Returns whether the cell belongs to an `OSC 8` hyperlink.
    pub const fn has_hyperlink(self) -> bool {
        ((self.0 >> 45) & 1) != 0
    }

    /// Returns the semantic prompt role (`OSC 133`).
    pub const fn semantic_content(self) -> GhosttyCellSemanticContent {
        match (self.0 >> 46) & 0b11 {
            1 => GhosttyCellSemanticContent::Input,
            2 => GhosttyCellSemanticContent::Prompt,
            _ => GhosttyCellSemanticContent::Output,
        }
    }
}

/// Packed 64-bit terminal row handle (`typedef uint64_t GhosttyRow` in `ghostty/vt/screen.h`).
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GhosttyRow(pub u64);

impl GhosttyRow {
    /// Creates a row bitfield from its boolean flags.
    pub const fn from_flags(wrap: bool, wrap_continuation: bool, dirty: bool) -> Self {
        let mut raw = 0_u64;
        if wrap {
            raw |= 1 << 32;
        }
        if wrap_continuation {
            raw |= 1 << 33;
        }
        if dirty {
            raw |= 1 << 40;
        }
        Self(raw)
    }

    /// Whether this row soft-wraps onto the next row.
    pub const fn wrap(self) -> bool {
        ((self.0 >> 32) & 1) != 0
    }

    /// Whether this row continues a soft-wrapped predecessor.
    pub const fn wrap_continuation(self) -> bool {
        ((self.0 >> 33) & 1) != 0
    }

    /// Whether this row requires redraw.
    pub const fn dirty(self) -> bool {
        ((self.0 >> 40) & 1) != 0
    }
}
