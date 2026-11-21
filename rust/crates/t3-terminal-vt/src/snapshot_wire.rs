//! Low-level `"T3VT"` wire serialization helpers, `CRC-32`, and `FNV-1a` digests.

use crate::abi::GhosttyStyleFlags;
use crate::error::VtError;
use crate::palette::{AnsiPalette256, Rgba};
use crate::snapshot::{MAX_SNAPSHOT_CELLS, SnapshotCell, T3VT_MAGIC, TerminalSnapshot};

/// Computes the standard IEEE `802.3` `CRC-32` digest over `bytes`.
pub fn crc32_ieee(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFF_u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// Computes the 64-bit `FNV-1a` digest over `bytes`.
pub fn fnv1a_64(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0100_0000_01b3;
    let mut hash = OFFSET_BASIS;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

pub(crate) fn validate_dimensions(cols: u16, rows: u16) -> Result<usize, VtError> {
    if cols == 0 || rows == 0 {
        return Err(VtError::InvalidDimensions { cols, rows });
    }
    let count = usize::from(cols)
        .checked_mul(usize::from(rows))
        .ok_or(VtError::CellCountOverflow { cols, rows })?;
    if count > MAX_SNAPSHOT_CELLS {
        return Err(VtError::CellCountOverflow { cols, rows });
    }
    Ok(count)
}

pub(crate) fn write_header(
    out: &mut Vec<u8>,
    version: u16,
    cols: u16,
    rows: u16,
    snapshot: &TerminalSnapshot,
    flags: u8,
) {
    out.extend_from_slice(&T3VT_MAGIC);
    out.extend_from_slice(&version.to_le_bytes());
    out.extend_from_slice(&cols.to_le_bytes());
    out.extend_from_slice(&rows.to_le_bytes());
    out.extend_from_slice(&snapshot.cursor_x.to_le_bytes());
    out.extend_from_slice(&snapshot.cursor_y.to_le_bytes());
    out.push(u8::from(snapshot.cursor_visible));
    out.push(snapshot.cursor_style.to_u8());
    out.push(u8::from(snapshot.cursor_blinking));
    out.push(flags);
    out.extend_from_slice(&snapshot.foreground.to_argb_u32().to_le_bytes());
    out.extend_from_slice(&snapshot.background.to_argb_u32().to_le_bytes());
    out.extend_from_slice(&snapshot.cursor_color.to_argb_u32().to_le_bytes());
}

pub(crate) fn write_cells(out: &mut Vec<u8>, snapshot: &TerminalSnapshot, cell_count: usize) {
    let blank = SnapshotCell::blank(snapshot.foreground, snapshot.background);
    for idx in 0..cell_count {
        let cell = snapshot.cells.get(idx).unwrap_or(&blank);
        out.extend_from_slice(&cell.foreground.to_argb_u32().to_le_bytes());
        out.extend_from_slice(&cell.background.to_argb_u32().to_le_bytes());
        out.extend_from_slice(&cell.flags.bits().to_le_bytes());
        let text_bytes = clamp_utf8_prefix(&cell.text, usize::from(u16::MAX));
        let text_len = u16::try_from(text_bytes.len()).unwrap_or(u16::MAX);
        out.extend_from_slice(&text_len.to_le_bytes());
        out.extend_from_slice(text_bytes);
    }
}

pub(crate) fn decode_palette(bytes: &[u8], offset: &mut usize) -> Result<AnsiPalette256, VtError> {
    let count = read_u16(bytes, *offset)?;
    *offset += 2;
    if usize::from(count) > crate::PALETTE_LEN {
        return Err(VtError::InvalidPaletteCount(count));
    }
    let mut palette = AnsiPalette256::new();
    let mut seen = [false; crate::PALETTE_LEN];
    for _ in 0..count {
        let entry_end = *offset + 5;
        if entry_end > bytes.len() {
            return Err(VtError::TruncatedBuffer {
                expected_min: entry_end,
                actual: bytes.len(),
            });
        }
        let idx = bytes[*offset];
        if seen[usize::from(idx)] {
            return Err(VtError::DuplicatePaletteIndex(idx));
        }
        seen[usize::from(idx)] = true;
        let color = Rgba::from_argb_u32(read_u32(bytes, *offset + 1)?);
        palette.set_override(idx, Some(color));
        *offset = entry_end;
    }
    Ok(palette)
}

pub(crate) fn decode_cells(
    bytes: &[u8],
    offset: &mut usize,
    cell_count: usize,
) -> Result<Vec<SnapshotCell>, VtError> {
    let mut cells = Vec::with_capacity(cell_count);
    for _ in 0..cell_count {
        let cell_header_end = *offset + 12;
        if cell_header_end > bytes.len() {
            return Err(VtError::TruncatedBuffer {
                expected_min: cell_header_end,
                actual: bytes.len(),
            });
        }
        let cell_foreground = Rgba::from_argb_u32(read_u32(bytes, *offset)?);
        let cell_background = Rgba::from_argb_u32(read_u32(bytes, *offset + 4)?);
        let flags = GhosttyStyleFlags(read_u16(bytes, *offset + 8)?);
        let text_len = usize::from(read_u16(bytes, *offset + 10)?);
        *offset = cell_header_end;
        let text_end = offset.saturating_add(text_len);
        if text_end > bytes.len() {
            return Err(VtError::TruncatedBuffer {
                expected_min: text_end,
                actual: bytes.len(),
            });
        }
        let text = std::str::from_utf8(&bytes[*offset..text_end])
            .map_err(|_| VtError::InvalidUtf8)?
            .to_owned();
        *offset = text_end;
        cells.push(SnapshotCell {
            foreground: cell_foreground,
            background: cell_background,
            flags,
            text,
        });
    }
    Ok(cells)
}

fn clamp_utf8_prefix(text: &str, max_bytes: usize) -> &[u8] {
    if text.len() <= max_bytes {
        return text.as_bytes();
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text.as_bytes()[..end]
}

pub(crate) fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, VtError> {
    let slice = bytes
        .get(offset..offset + 2)
        .ok_or(VtError::TruncatedBuffer {
            expected_min: offset + 2,
            actual: bytes.len(),
        })?;
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}

pub(crate) fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, VtError> {
    let slice = bytes
        .get(offset..offset + 4)
        .ok_or(VtError::TruncatedBuffer {
            expected_min: offset + 4,
            actual: bytes.len(),
        })?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

pub(crate) fn read_bool(byte: u8, field: &'static str) -> Result<bool, VtError> {
    match byte {
        0 => Ok(false),
        1 => Ok(true),
        value => Err(VtError::InvalidBooleanField { field, value }),
    }
}
