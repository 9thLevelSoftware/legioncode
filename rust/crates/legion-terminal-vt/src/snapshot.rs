//! `"LCVT"` binary terminal snapshot codec with `CRC-32` integrity and legacy `v1` interop.
//!
//! Layout (`little-endian`, matching `legion_terminal_jni.cpp` and `TerminalFrame.kt`):
//! - Header (`30` bytes):
//!   - `0..4`: magic `b"LCVT"` (`0x5456_434c`)
//!   - `4..6`: `version: u16` (`1` = legacy frame without palette/trailer; `2` = sparse palette + `CRC-32`)
//!   - `6..8`: `cols: u16`
//!   - `8..10`: `rows: u16`
//!   - `10..12`: `cursor_x: u16`
//!   - `12..14`: `cursor_y: u16`
//!   - `14`: `cursor_visible: u8` (`0` or `1`)
//!   - `15`: `cursor_style: u8` (`0..=3`, [`GhosttyCursorShape`])
//!   - `16`: `cursor_blinking: u8` (`0` or `1`)
//!   - `17`: `flags: u8` (`0` in `v1`; bit `0` = `alternate_screen` in `v2`)
//!   - `18..22`: `foreground: u32` (`0xAARRGGBB`)
//!   - `22..26`: `background: u32` (`0xAARRGGBB`)
//!   - `26..30`: `cursor_color: u32` (`0xAARRGGBB`)
//! - `v2` palette table: `override_count: u16` (`0..=256`), followed by `(index: u8, argb: u32)` pairs.
//! - Cell stream (`cols * rows` entries): `fg: u32`, `bg: u32`, `flags: u16`, `text_len: u16`, `text`.
//! - `v2` trailer: `crc32: u32` (IEEE `802.3` over all preceding bytes).

use serde::{Deserialize, Serialize};

use crate::abi::{GhosttyCursorShape, GhosttyStyleFlags};
use crate::error::VtError;
use crate::palette::{AnsiPalette256, Rgba};
pub use crate::snapshot_wire::{crc32_ieee, fnv1a_64};
use crate::snapshot_wire::{
    decode_cells, decode_palette, read_bool, read_u16, read_u32, validate_dimensions, write_cells,
    write_header,
};

/// Four-byte `"LCVT"` snapshot magic (`0x5456_434c` in little-endian).
pub const LCVT_MAGIC: [u8; 4] = *b"LCVT";
/// Legacy Android `JNI` / `TerminalFrame.kt` snapshot format version.
pub const LCVT_VERSION_V1: u16 = 1;
/// Integrity-checked snapshot version carrying sparse palette overrides and `CRC-32`.
pub const LCVT_VERSION_V2: u16 = 2;
/// Fixed header length in bytes.
pub const LCVT_HEADER_BYTES: usize = 30;
/// Maximum permitted cell grid count (`1024 * 512`) to prevent allocation bombs on malformed input.
pub const MAX_SNAPSHOT_CELLS: usize = 1024 * 512;

const V2_FLAG_ALTERNATE_SCREEN: u8 = 1 << 0;

/// One rendered cell inside a [`TerminalSnapshot`].
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SnapshotCell {
    /// Resolved cell foreground color (`0xAARRGGBB` on the wire).
    pub foreground: Rgba,
    /// Resolved cell background color (`0xAARRGGBB` on the wire).
    pub background: Rgba,
    /// Visual attribute bitflags ([`GhosttyStyleFlags`]).
    pub flags: GhosttyStyleFlags,
    /// UTF-8 grapheme cluster text (`""` for blank cells or wide spacer tails).
    pub text: String,
}

impl SnapshotCell {
    /// Creates an empty styled cell with no text.
    pub fn blank(foreground: Rgba, background: Rgba) -> Self {
        Self {
            foreground,
            background,
            flags: GhosttyStyleFlags::NONE,
            text: String::new(),
        }
    }
}

/// Complete serializable terminal viewport snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSnapshot {
    /// Viewport width in columns.
    pub cols: u16,
    /// Viewport height in rows.
    pub rows: u16,
    /// Viewport cursor column (`0xFFFF` when cursor is outside viewport).
    pub cursor_x: u16,
    /// Viewport cursor row (`0xFFFF` when cursor is outside viewport).
    pub cursor_y: u16,
    /// Whether the cursor is visible in the viewport.
    pub cursor_visible: bool,
    /// Visual cursor shape.
    pub cursor_style: GhosttyCursorShape,
    /// Whether cursor blink is active.
    pub cursor_blinking: bool,
    /// Whether the alternate screen is active.
    pub alternate_screen: bool,
    /// Effective terminal foreground color.
    pub foreground: Rgba,
    /// Effective terminal background color.
    pub background: Rgba,
    /// Effective terminal cursor color.
    pub cursor_color: Rgba,
    /// Active sparse 256-color ANSI palette overrides.
    pub palette: AnsiPalette256,
    /// Row-major viewport cells (`cols * rows`).
    pub cells: Vec<SnapshotCell>,
}

impl TerminalSnapshot {
    /// Builds a validated snapshot with blank cells initialized to `foreground` and `background`.
    pub fn try_new(
        cols: u16,
        rows: u16,
        foreground: Rgba,
        background: Rgba,
        cursor_color: Rgba,
    ) -> Result<Self, VtError> {
        let cell_count = validate_dimensions(cols, rows)?;
        Ok(Self {
            cols,
            rows,
            cursor_x: 0,
            cursor_y: 0,
            cursor_visible: true,
            cursor_style: GhosttyCursorShape::Block,
            cursor_blinking: true,
            alternate_screen: false,
            foreground,
            background,
            cursor_color,
            palette: AnsiPalette256::new(),
            cells: vec![SnapshotCell::blank(foreground, background); cell_count],
        })
    }
}

/// Encodes `snapshot` into the `"LCVT"` v2 binary format with sparse palette and `CRC-32`
/// integrity trailer.
pub fn encode_lcvt(snapshot: &TerminalSnapshot) -> Vec<u8> {
    let cols = snapshot.cols.max(1);
    let rows = snapshot.rows.max(1);
    let cell_count = usize::from(cols)
        .saturating_mul(usize::from(rows))
        .min(MAX_SNAPSHOT_CELLS);
    let overrides = snapshot.palette.override_pairs();

    let mut out =
        Vec::with_capacity(LCVT_HEADER_BYTES + 2 + overrides.len() * 5 + cell_count * 14 + 4);
    let flags = if snapshot.alternate_screen {
        V2_FLAG_ALTERNATE_SCREEN
    } else {
        0
    };
    write_header(&mut out, LCVT_VERSION_V2, cols, rows, snapshot, flags);

    let override_count = u16::try_from(overrides.len().min(256)).unwrap_or(256);
    out.extend_from_slice(&override_count.to_le_bytes());
    for (index, color) in overrides.into_iter().take(usize::from(override_count)) {
        out.push(index);
        out.extend_from_slice(&color.to_argb_u32().to_le_bytes());
    }

    write_cells(&mut out, snapshot, cell_count);
    let checksum = crc32_ieee(&out);
    out.extend_from_slice(&checksum.to_le_bytes());
    out
}

/// Validates dimensions and cell count strictly before encoding `"LCVT"` v2 bytes.
pub fn try_encode_lcvt(snapshot: &TerminalSnapshot) -> Result<Vec<u8>, VtError> {
    let expected = validate_dimensions(snapshot.cols, snapshot.rows)?;
    if snapshot.cells.len() != expected {
        return Err(VtError::CellCountMismatch {
            expected,
            actual: snapshot.cells.len(),
        });
    }
    Ok(encode_lcvt(snapshot))
}

/// Encodes `snapshot` into the legacy `"LCVT"` v1 format consumed by Android `TerminalFrame.kt`.
pub fn encode_lcvt_v1(snapshot: &TerminalSnapshot) -> Vec<u8> {
    let cols = snapshot.cols.max(1);
    let rows = snapshot.rows.max(1);
    let cell_count = usize::from(cols)
        .saturating_mul(usize::from(rows))
        .min(MAX_SNAPSHOT_CELLS);
    let mut out = Vec::with_capacity(LCVT_HEADER_BYTES + cell_count * 14);
    write_header(&mut out, LCVT_VERSION_V1, cols, rows, snapshot, 0);
    write_cells(&mut out, snapshot, cell_count);
    out
}

/// Decodes a `"LCVT"` binary frame (`v1` legacy or `v2` integrity-checked), rejecting truncated,
/// corrupted, or malformed buffers via [`VtError`].
pub fn decode_lcvt(bytes: &[u8]) -> Result<TerminalSnapshot, VtError> {
    if bytes.len() < LCVT_HEADER_BYTES {
        return Err(VtError::TruncatedBuffer {
            expected_min: LCVT_HEADER_BYTES,
            actual: bytes.len(),
        });
    }
    let magic = [bytes[0], bytes[1], bytes[2], bytes[3]];
    if magic != LCVT_MAGIC {
        return Err(VtError::InvalidMagic { found: magic });
    }
    let version = read_u16(bytes, 4)?;
    match version {
        LCVT_VERSION_V1 => decode_payload(bytes, version, bytes.len()),
        LCVT_VERSION_V2 => {
            let min_v2 = LCVT_HEADER_BYTES + 2 + 4;
            if bytes.len() < min_v2 {
                return Err(VtError::TruncatedBuffer {
                    expected_min: min_v2,
                    actual: bytes.len(),
                });
            }
            let payload_end = bytes.len() - 4;
            let expected = read_u32(bytes, payload_end)?;
            let actual = crc32_ieee(&bytes[..payload_end]);
            if expected != actual {
                return Err(VtError::ChecksumMismatch { expected, actual });
            }
            decode_payload(bytes, version, payload_end)
        }
        other => Err(VtError::UnsupportedVersion(other)),
    }
}

fn decode_payload(
    bytes: &[u8],
    version: u16,
    payload_end: usize,
) -> Result<TerminalSnapshot, VtError> {
    let cols = read_u16(bytes, 6)?;
    let rows = read_u16(bytes, 8)?;
    let cell_count = validate_dimensions(cols, rows)?;
    let cursor_x = read_u16(bytes, 10)?;
    let cursor_y = read_u16(bytes, 12)?;
    let cursor_visible = read_bool(bytes[14], "cursor_visible")?;
    let cursor_style =
        GhosttyCursorShape::from_u8(bytes[15]).ok_or(VtError::InvalidCursorStyle(bytes[15]))?;
    let cursor_blinking = read_bool(bytes[16], "cursor_blinking")?;
    let header_flags = bytes[17];
    let alternate_screen = if version == LCVT_VERSION_V2 {
        if (header_flags & !V2_FLAG_ALTERNATE_SCREEN) != 0 {
            return Err(VtError::InvalidBooleanField {
                field: "header_flags",
                value: header_flags,
            });
        }
        (header_flags & V2_FLAG_ALTERNATE_SCREEN) != 0
    } else {
        false
    };
    let foreground = Rgba::from_argb_u32(read_u32(bytes, 18)?);
    let background = Rgba::from_argb_u32(read_u32(bytes, 22)?);
    let cursor_color = Rgba::from_argb_u32(read_u32(bytes, 26)?);

    let mut offset = LCVT_HEADER_BYTES;
    let palette = if version == LCVT_VERSION_V2 {
        decode_palette(&bytes[..payload_end], &mut offset)?
    } else {
        AnsiPalette256::new()
    };
    let cells = decode_cells(&bytes[..payload_end], &mut offset, cell_count)?;

    if offset != payload_end {
        return Err(VtError::TrailingBytes(payload_end - offset));
    }

    Ok(TerminalSnapshot {
        cols,
        rows,
        cursor_x,
        cursor_y,
        cursor_visible,
        cursor_style,
        cursor_blinking,
        alternate_screen,
        foreground,
        background,
        cursor_color,
        palette,
        cells,
    })
}
