//! Powerline glyph classification (`U+E0B0..=U+E0BF`) and full-cell geometry flags (`2e6b05011f` #24888).
//!
//! Fallback fonts inset or scale Powerline private-use glyphs, leaving hairline seams between
//! adjacent cells. Terminal renderers draw these glyphs directly across the full cell bounds and
//! break text runs whenever a cell holds a Powerline code point.

use serde::{Deserialize, Serialize};

/// First Powerline separator/cap/slant code point (`U+E0B0`).
pub const POWERLINE_RANGE_START: char = '\u{E0B0}';
/// Last Powerline separator/cap/slant code point (`U+E0BF`).
pub const POWERLINE_RANGE_END: char = '\u{E0BF}';

/// Normalized `(x, y)` coordinates in `[0.0, 1.0]` spanning the full terminal cell box.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CellPoint {
    pub x: f32,
    pub y: f32,
}

impl CellPoint {
    const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// Which cell edge holds the vertical base of a half-ellipse round cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PowerlineFlatEdge {
    Left,
    Right,
}

/// Semantic family for one of the 16 Powerline glyphs in `U+E0B0..=U+E0BF`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PowerlineGlyphKind {
    TriangleRight,
    TriangleLeft,
    RoundCapRight,
    RoundCapLeft,
    LowerLeftSlant,
    LowerRightSlant,
    UpperLeftSlant,
    UpperRightSlant,
}

/// Geometric drawing primitive spanning a single terminal cell (`renderer.ts:25-44`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PowerlineShape {
    /// Closed polygon filled with cell foreground color across full cell bounds.
    Fill { points: &'static [CellPoint] },
    /// Open polyline stroked at `1px` and clipped to the cell rectangle.
    Stroke { points: &'static [CellPoint] },
    /// Half-ellipse round cap anchored to `flat_edge`.
    Cap {
        flat_edge: PowerlineFlatEdge,
        filled: bool,
    },
}

/// Renderer geometry flags emitted for every Powerline glyph cell (`#24888`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[expect(
    clippy::struct_excessive_bools,
    reason = "renderer cell geometry flags serialize directly as boolean properties"
)]
pub struct PowerlineCellFlags {
    /// Drawn directly to full cell edges instead of via font glyph fallback.
    pub full_cell_geometry: bool,
    /// Filled solid region (`true` for even code points) vs `1px` stroked outline (`false`).
    pub filled: bool,
    /// Renderer must clip drawing to the cell rect (`!filled`).
    pub clip_to_cell: bool,
    /// Filled round caps overlap the adjacent segment edge to avoid fractional-DPR seams.
    pub needs_seam_overlap: bool,
    /// Cell breaks adjacent text runs (`ghosttyTextRunEnd`).
    pub breaks_text_run: bool,
}

/// Complete classification and normalized cell geometry for a `U+E0B0..=U+E0BF` Powerline glyph.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PowerlineGlyph {
    pub codepoint: char,
    pub kind: PowerlineGlyphKind,
    pub shape: PowerlineShape,
    pub flags: PowerlineCellFlags,
}

const TRI_RIGHT: &[CellPoint] = &[
    CellPoint::new(0.0, 0.0),
    CellPoint::new(1.0, 0.5),
    CellPoint::new(0.0, 1.0),
];
const TRI_LEFT: &[CellPoint] = &[
    CellPoint::new(1.0, 0.0),
    CellPoint::new(0.0, 0.5),
    CellPoint::new(1.0, 1.0),
];
const LOWER_LEFT_FILL: &[CellPoint] = &[
    CellPoint::new(0.0, 0.0),
    CellPoint::new(1.0, 1.0),
    CellPoint::new(0.0, 1.0),
];
const LOWER_LEFT_STROKE: &[CellPoint] = &[CellPoint::new(0.0, 0.0), CellPoint::new(1.0, 1.0)];
const LOWER_RIGHT_FILL: &[CellPoint] = &[
    CellPoint::new(1.0, 0.0),
    CellPoint::new(1.0, 1.0),
    CellPoint::new(0.0, 1.0),
];
const DIAG_BL_TR: &[CellPoint] = &[CellPoint::new(1.0, 0.0), CellPoint::new(0.0, 1.0)];
const UPPER_LEFT_FILL: &[CellPoint] = &[
    CellPoint::new(0.0, 0.0),
    CellPoint::new(1.0, 0.0),
    CellPoint::new(0.0, 1.0),
];
const UPPER_RIGHT_FILL: &[CellPoint] = &[
    CellPoint::new(0.0, 0.0),
    CellPoint::new(1.0, 0.0),
    CellPoint::new(1.0, 1.0),
];

const fn glyph(
    codepoint: char,
    kind: PowerlineGlyphKind,
    shape: PowerlineShape,
    filled: bool,
    needs_seam_overlap: bool,
) -> PowerlineGlyph {
    PowerlineGlyph {
        codepoint,
        kind,
        shape,
        flags: PowerlineCellFlags {
            full_cell_geometry: true,
            filled,
            clip_to_cell: !filled,
            needs_seam_overlap,
            breaks_text_run: true,
        },
    }
}

/// All 16 Powerline glyphs `U+E0B0..=U+E0BF` in code point order.
pub const POWERLINE_GLYPHS: [PowerlineGlyph; 16] = [
    glyph(
        '\u{E0B0}',
        PowerlineGlyphKind::TriangleRight,
        PowerlineShape::Fill { points: TRI_RIGHT },
        true,
        false,
    ),
    glyph(
        '\u{E0B1}',
        PowerlineGlyphKind::TriangleRight,
        PowerlineShape::Stroke { points: TRI_RIGHT },
        false,
        false,
    ),
    glyph(
        '\u{E0B2}',
        PowerlineGlyphKind::TriangleLeft,
        PowerlineShape::Fill { points: TRI_LEFT },
        true,
        false,
    ),
    glyph(
        '\u{E0B3}',
        PowerlineGlyphKind::TriangleLeft,
        PowerlineShape::Stroke { points: TRI_LEFT },
        false,
        false,
    ),
    glyph(
        '\u{E0B4}',
        PowerlineGlyphKind::RoundCapRight,
        PowerlineShape::Cap {
            flat_edge: PowerlineFlatEdge::Left,
            filled: true,
        },
        true,
        true,
    ),
    glyph(
        '\u{E0B5}',
        PowerlineGlyphKind::RoundCapRight,
        PowerlineShape::Cap {
            flat_edge: PowerlineFlatEdge::Left,
            filled: false,
        },
        false,
        false,
    ),
    glyph(
        '\u{E0B6}',
        PowerlineGlyphKind::RoundCapLeft,
        PowerlineShape::Cap {
            flat_edge: PowerlineFlatEdge::Right,
            filled: true,
        },
        true,
        true,
    ),
    glyph(
        '\u{E0B7}',
        PowerlineGlyphKind::RoundCapLeft,
        PowerlineShape::Cap {
            flat_edge: PowerlineFlatEdge::Right,
            filled: false,
        },
        false,
        false,
    ),
    glyph(
        '\u{E0B8}',
        PowerlineGlyphKind::LowerLeftSlant,
        PowerlineShape::Fill {
            points: LOWER_LEFT_FILL,
        },
        true,
        false,
    ),
    glyph(
        '\u{E0B9}',
        PowerlineGlyphKind::LowerLeftSlant,
        PowerlineShape::Stroke {
            points: LOWER_LEFT_STROKE,
        },
        false,
        false,
    ),
    glyph(
        '\u{E0BA}',
        PowerlineGlyphKind::LowerRightSlant,
        PowerlineShape::Fill {
            points: LOWER_RIGHT_FILL,
        },
        true,
        false,
    ),
    glyph(
        '\u{E0BB}',
        PowerlineGlyphKind::LowerRightSlant,
        PowerlineShape::Stroke { points: DIAG_BL_TR },
        false,
        false,
    ),
    glyph(
        '\u{E0BC}',
        PowerlineGlyphKind::UpperLeftSlant,
        PowerlineShape::Fill {
            points: UPPER_LEFT_FILL,
        },
        true,
        false,
    ),
    glyph(
        '\u{E0BD}',
        PowerlineGlyphKind::UpperLeftSlant,
        PowerlineShape::Stroke { points: DIAG_BL_TR },
        false,
        false,
    ),
    glyph(
        '\u{E0BE}',
        PowerlineGlyphKind::UpperRightSlant,
        PowerlineShape::Fill {
            points: UPPER_RIGHT_FILL,
        },
        true,
        false,
    ),
    glyph(
        '\u{E0BF}',
        PowerlineGlyphKind::UpperRightSlant,
        PowerlineShape::Stroke {
            points: LOWER_LEFT_STROKE,
        },
        false,
        false,
    ),
];

/// Returns `true` when `ch` is in the Powerline glyph range `U+E0B0..=U+E0BF`.
pub const fn is_powerline_char(ch: char) -> bool {
    (ch as u32) >= (POWERLINE_RANGE_START as u32) && (ch as u32) <= (POWERLINE_RANGE_END as u32)
}

/// Returns `true` when a cell's text consists solely of a single `U+E0B0..=U+E0BF` glyph.
pub fn is_powerline_cell_text(text: &str) -> bool {
    let mut chars = text.chars();
    matches!((chars.next(), chars.next()), (Some(ch), None) if is_powerline_char(ch))
}

/// Classifies `ch` into its full-cell [`PowerlineGlyph`] geometry, or `None` outside `U+E0B0..=U+E0BF`.
pub const fn classify_powerline_char(ch: char) -> Option<PowerlineGlyph> {
    let cp = ch as u32;
    let start = POWERLINE_RANGE_START as u32;
    let end = POWERLINE_RANGE_END as u32;
    if cp < start || cp > end {
        return None;
    }
    Some(POWERLINE_GLYPHS[(cp - start) as usize])
}

/// Classifies a cell's grapheme string when it is a single `U+E0B0..=U+E0BF` Powerline glyph.
pub fn classify_powerline_text(text: &str) -> Option<PowerlineGlyph> {
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(ch), None) => classify_powerline_char(ch),
        _ => None,
    }
}
