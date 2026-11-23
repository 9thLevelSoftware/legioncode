//! Core types and font-style bitmask for [`crate::mobile_diff`] (`BUG-22`).

use std::collections::BTreeMap;
use std::ops::{BitAnd, BitAndAssign, BitOr, BitOrAssign};

use serde::{Deserialize, Serialize};

/// `TextMate` / Shiki compatible bitmask for syntax token font styling (`BUG-22`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FontStyleFlags(u8);

impl FontStyleFlags {
    /// Normal upright regular-weight text.
    pub const NONE: Self = Self(0);
    /// Italic slant (`1 << 0 = 1`).
    pub const ITALIC: Self = Self(legion_client_core::diff_markdown::FontStyleFlags::ITALIC);
    /// Bold weight (`1 << 1 = 2`).
    pub const BOLD: Self = Self(legion_client_core::diff_markdown::FontStyleFlags::BOLD);
    /// Underline decoration (`1 << 2 = 4`).
    pub const UNDERLINE: Self = Self(legion_client_core::diff_markdown::FontStyleFlags::UNDERLINE);
    /// Strikethrough decoration (`1 << 3 = 8`).
    pub const STRIKETHROUGH: Self = Self(8);

    const KNOWN_MASK: u8 =
        Self::ITALIC.0 | Self::BOLD.0 | Self::UNDERLINE.0 | Self::STRIKETHROUGH.0;

    /// Constructs flags from a raw bitmask, discarding unknown high bits.
    pub const fn from_bits_truncate(bits: u8) -> Self {
        Self(bits & Self::KNOWN_MASK)
    }

    /// Returns the raw `u8` bitmask (`0..=15`).
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Returns `true` when every bit in `other` is enabled in `self`.
    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    /// Returns `true` when no font style bits are set.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Returns `true` when `ITALIC` is set.
    pub const fn is_italic(self) -> bool {
        self.contains(Self::ITALIC)
    }

    /// Returns `true` when `BOLD` is set.
    pub const fn is_bold(self) -> bool {
        self.contains(Self::BOLD)
    }

    /// Returns `true` when `UNDERLINE` is set.
    pub const fn is_underline(self) -> bool {
        self.contains(Self::UNDERLINE)
    }

    /// Returns `true` when `STRIKETHROUGH` is set.
    pub const fn is_strikethrough(self) -> bool {
        self.contains(Self::STRIKETHROUGH)
    }

    /// Combines two flag sets.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

impl From<u8> for FontStyleFlags {
    fn from(bits: u8) -> Self {
        Self::from_bits_truncate(bits)
    }
}

impl BitOr for FontStyleFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for FontStyleFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl BitAnd for FontStyleFlags {
    type Output = Self;

    fn bitand(self, rhs: Self) -> Self::Output {
        Self(self.0 & rhs.0)
    }
}

impl BitAndAssign for FontStyleFlags {
    fn bitand_assign(&mut self, rhs: Self) {
        self.0 &= rhs.0;
    }
}

/// Raw syntax token input matching mobile `ReviewDiffNativeToken` payloads.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyntaxTokenInput {
    /// Token text slice.
    pub content: String,
    /// Optional hex color string (e.g. `"#ff0000"`).
    pub color: Option<String>,
    /// Bitmask of [`FontStyleFlags`].
    #[serde(default)]
    pub font_style: FontStyleFlags,
    /// Whether font ligatures are enabled (defaults to `false` for monospace code alignment).
    #[serde(default)]
    pub ligatures_enabled: bool,
    /// Additional non-color attributes preserved across highlight splitting.
    #[serde(default)]
    pub attributes: BTreeMap<String, String>,
}

/// Decorated UTF-16 attributed run emitted for iOS/Android/Web diff line rendering (`BUG-22`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttributedDiffRun {
    /// Inclusive UTF-16 start offset on the diff line.
    pub start: u32,
    /// Exclusive UTF-16 end offset on the diff line.
    pub end: u32,
    /// Text slice covered by `[start, end)` (empty for range-only runs).
    pub text: String,
    /// Optional foreground color (e.g. `"#ff0000"`).
    pub foreground_color: Option<String>,
    /// Optional explicit background color (e.g. cut-line warning note or applied highlight).
    pub background_color: Option<String>,
    /// Preserved syntax font styling (`BOLD`, `ITALIC`, `UNDERLINE`, `STRIKETHROUGH`).
    pub font_style: FontStyleFlags,
    /// `true` when this run overlaps an intra-line word-diff highlight range.
    pub word_highlighted: bool,
    /// Ligature flag (`false` matches `.ligature: 0` in `LegionReviewDiffView.swift`).
    pub ligatures_enabled: bool,
    /// `true` when this run belongs to a cut-line warning note rather than source code.
    pub is_note: bool,
    /// Non-color metadata attributes preserved verbatim when splitting/merging runs.
    #[serde(default)]
    pub attributes: BTreeMap<String, String>,
}

impl AttributedDiffRun {
    /// Creates a plain attributed run covering UTF-16 offsets `[start, end)`.
    pub fn new(start: u32, end: u32, text: impl Into<String>) -> Self {
        Self {
            start,
            end,
            text: text.into(),
            foreground_color: None,
            background_color: None,
            font_style: FontStyleFlags::NONE,
            word_highlighted: false,
            ligatures_enabled: false,
            is_note: false,
            attributes: BTreeMap::new(),
        }
    }

    /// Sets `foreground_color` using a builder pattern.
    #[must_use]
    pub fn with_foreground_color(mut self, color: impl Into<String>) -> Self {
        self.foreground_color = Some(color.into());
        self
    }

    /// Sets `background_color` using a builder pattern.
    #[must_use]
    pub fn with_background_color(mut self, color: impl Into<String>) -> Self {
        self.background_color = Some(color.into());
        self
    }

    /// Sets `font_style` using a builder pattern.
    #[must_use]
    pub fn with_font_style(mut self, font_style: FontStyleFlags) -> Self {
        self.font_style = font_style;
        self
    }

    /// Sets `ligatures_enabled` using a builder pattern.
    #[must_use]
    pub fn with_ligatures(mut self, enabled: bool) -> Self {
        self.ligatures_enabled = enabled;
        self
    }

    /// Marks whether this run is a cut-line warning note.
    #[must_use]
    pub fn with_note(mut self, is_note: bool) -> Self {
        self.is_note = is_note;
        self
    }

    /// Attaches one arbitrary non-color attribute preserved across highlight splits.
    #[must_use]
    pub fn with_attribute(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.attributes.insert(key.into(), value.into());
        self
    }

    /// Returns `true` when two runs share identical visual and non-color attributes.
    pub fn has_same_style(&self, other: &Self) -> bool {
        self.foreground_color == other.foreground_color
            && self.background_color == other.background_color
            && self.font_style == other.font_style
            && self.word_highlighted == other.word_highlighted
            && self.ligatures_enabled == other.ligatures_enabled
            && self.is_note == other.is_note
            && self.attributes == other.attributes
    }
}
