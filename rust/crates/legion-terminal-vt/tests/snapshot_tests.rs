//! Integration and property tests for `"LCVT"` binary snapshot encoding, legacy `v1` interop,
//! `CRC-32` integrity verification, and strict bounds rejection.

use legion_terminal_vt::abi::{GhosttyCursorShape, GhosttyStyleFlags};
use legion_terminal_vt::error::VtError;
use legion_terminal_vt::palette::{AnsiPalette256, Rgba};
use legion_terminal_vt::snapshot::{
    LCVT_HEADER_BYTES, LCVT_MAGIC, LCVT_VERSION_V1, LCVT_VERSION_V2, SnapshotCell,
    TerminalSnapshot, decode_lcvt, encode_lcvt, encode_lcvt_v1, try_encode_lcvt,
};
use proptest::prelude::*;

#[test]
fn lcvt_v2_round_trips_sparse_palette_and_unicode_cells() -> Result<(), String> {
    let fg = Rgba::rgb(0xD1, 0xD5, 0xDA);
    let bg = Rgba::rgb(0x24, 0x29, 0x2E);
    let cursor = Rgba::rgb(0x00, 0x9F, 0xFF);
    let mut snapshot =
        TerminalSnapshot::try_new(3, 2, fg, bg, cursor).map_err(|e| e.to_string())?;
    snapshot.cursor_x = 2;
    snapshot.cursor_y = 1;
    snapshot.cursor_visible = true;
    snapshot.cursor_style = GhosttyCursorShape::Bar;
    snapshot.cursor_blinking = false;
    snapshot.alternate_screen = true;
    snapshot.palette.apply_theme_overrides(&[
        (1, Rgba::rgb(0xFF, 0x2E, 0x3F)),
        (200, Rgba::rgb(12, 34, 56)),
    ]);
    snapshot.cells[0] = SnapshotCell {
        foreground: Rgba::rgb(255, 0, 0),
        background: bg,
        flags: GhosttyStyleFlags::BOLD | GhosttyStyleFlags::UNDERLINE,
        text: "e\u{0301}".to_owned(),
    };
    snapshot.cells[4] = SnapshotCell {
        foreground: fg,
        background: Rgba::rgb(1, 2, 3),
        flags: GhosttyStyleFlags::SELECTED,
        text: "🙂".to_owned(),
    };

    let bytes = encode_lcvt(&snapshot);
    assert_eq!(&bytes[..4], &LCVT_MAGIC);
    assert_eq!(u16::from_le_bytes([bytes[4], bytes[5]]), LCVT_VERSION_V2);
    let decoded = decode_lcvt(&bytes).map_err(|e| e.to_string())?;
    assert_eq!(decoded, snapshot);
    Ok(())
}

#[test]
fn lcvt_v1_interop_matches_legacy_jni_and_kotlin_wire_layout() -> Result<(), String> {
    let fg = Rgba::rgb(0xAA, 0xBB, 0xCC);
    let bg = Rgba::rgb(0x10, 0x20, 0x30);
    let cursor = Rgba::rgb(0x00, 0x9F, 0xFF);

    // Construct an exact legacy JNI buffer (30-byte header + 2 cells, no palette table/CRC).
    let mut legacy = Vec::new();
    legacy.extend_from_slice(&0x5456_434c_u32.to_le_bytes());
    legacy.extend_from_slice(&LCVT_VERSION_V1.to_le_bytes());
    legacy.extend_from_slice(&2_u16.to_le_bytes());
    legacy.extend_from_slice(&1_u16.to_le_bytes());
    legacy.extend_from_slice(&1_u16.to_le_bytes());
    legacy.extend_from_slice(&0_u16.to_le_bytes());
    legacy.push(1);
    legacy.push(GhosttyCursorShape::Underline.to_u8());
    legacy.push(0);
    legacy.push(0);
    legacy.extend_from_slice(&fg.to_argb_u32().to_le_bytes());
    legacy.extend_from_slice(&bg.to_argb_u32().to_le_bytes());
    legacy.extend_from_slice(&cursor.to_argb_u32().to_le_bytes());
    assert_eq!(legacy.len(), LCVT_HEADER_BYTES);

    // Cell 0: "hi" with bold flag.
    legacy.extend_from_slice(&fg.to_argb_u32().to_le_bytes());
    legacy.extend_from_slice(&bg.to_argb_u32().to_le_bytes());
    legacy.extend_from_slice(&GhosttyStyleFlags::BOLD.bits().to_le_bytes());
    legacy.extend_from_slice(&2_u16.to_le_bytes());
    legacy.extend_from_slice(b"hi");

    // Cell 1: blank.
    legacy.extend_from_slice(&fg.to_argb_u32().to_le_bytes());
    legacy.extend_from_slice(&bg.to_argb_u32().to_le_bytes());
    legacy.extend_from_slice(&0_u16.to_le_bytes());
    legacy.extend_from_slice(&0_u16.to_le_bytes());

    let decoded = decode_lcvt(&legacy).map_err(|e| e.to_string())?;
    assert_eq!(decoded.cols, 2);
    assert_eq!(decoded.rows, 1);
    assert_eq!(decoded.cursor_x, 1);
    assert_eq!(decoded.cursor_style, GhosttyCursorShape::Underline);
    assert_eq!(decoded.cells[0].text, "hi");
    assert_eq!(decoded.cells[1].text, "");
    assert_eq!(encode_lcvt_v1(&decoded), legacy);
    Ok(())
}

#[test]
fn lcvt_rejects_corrupted_truncated_and_malformed_buffers() -> Result<(), String> {
    let snap = TerminalSnapshot::try_new(2, 2, Rgba::WHITE, Rgba::BLACK, Rgba::WHITE)
        .map_err(|e| e.to_string())?;
    let valid = encode_lcvt(&snap);

    // Any bit-flip outside magic/version fails CRC-32 verification.
    for idx in 6..valid.len() {
        let mut tampered = valid.clone();
        tampered[idx] ^= 0x01;
        assert!(matches!(
            decode_lcvt(&tampered),
            Err(VtError::ChecksumMismatch { .. })
        ));
    }

    // Any truncation fails cleanly with Err.
    for cut in 0..valid.len() {
        assert!(decode_lcvt(&valid[..cut]).is_err());
    }

    // Bad magic and unsupported version.
    let mut bad_magic = valid.clone();
    bad_magic[0] = b'X';
    assert!(matches!(
        decode_lcvt(&bad_magic),
        Err(VtError::InvalidMagic { .. })
    ));

    let mut bad_ver = valid;
    bad_ver[4..6].copy_from_slice(&99_u16.to_le_bytes());
    assert_eq!(decode_lcvt(&bad_ver), Err(VtError::UnsupportedVersion(99)));

    // Malformed v1: huge grid dimension bomb (65535 x 65535) & invalid UTF-8 & trailing bytes.
    let mut bomb = encode_lcvt_v1(&snap);
    bomb[6..8].copy_from_slice(&u16::MAX.to_le_bytes());
    bomb[8..10].copy_from_slice(&u16::MAX.to_le_bytes());
    assert!(matches!(
        decode_lcvt(&bomb),
        Err(VtError::CellCountOverflow { .. })
    ));

    let mut invalid_utf8 = encode_lcvt_v1(&snap);
    invalid_utf8[LCVT_HEADER_BYTES + 10..LCVT_HEADER_BYTES + 12]
        .copy_from_slice(&1_u16.to_le_bytes());
    invalid_utf8.insert(LCVT_HEADER_BYTES + 12, 0xFF);
    assert_eq!(decode_lcvt(&invalid_utf8), Err(VtError::InvalidUtf8));

    let mut trailing = encode_lcvt_v1(&snap);
    trailing.push(0);
    assert_eq!(decode_lcvt(&trailing), Err(VtError::TrailingBytes(1)));

    // Strict try_encode_lcvt rejects mismatched cell count.
    let mut bad_snap = snap;
    bad_snap.cells.pop();
    assert!(matches!(
        try_encode_lcvt(&bad_snap),
        Err(VtError::CellCountMismatch { .. })
    ));
    Ok(())
}

proptest! {
    #[test]
    fn prop_lcvt_round_trips_and_rejects_truncation(
        cols in 1_u16..12,
        rows in 1_u16..8,
        cursor_x in 0_u16..20,
        cursor_y in 0_u16..20,
        cursor_visible in any::<bool>(),
        cursor_blinking in any::<bool>(),
        alternate_screen in any::<bool>(),
        style_idx in 0_u8..4,
        palette_pairs in proptest::collection::vec((any::<u8>(), any::<u32>()), 0..24),
        texts in proptest::collection::vec("([a-zA-Z0-9 ]|界|🙂){0,2}", 1..96),
    ) {
        let mut palette = AnsiPalette256::new();
        let overrides: Vec<(u8, Rgba)> = palette_pairs
            .iter()
            .map(|&(i, argb)| (i, Rgba::from_argb_u32(argb)))
            .collect();
        palette.apply_theme_overrides(&overrides);

        let count = usize::from(cols) * usize::from(rows);
        let cells = (0..count)
            .map(|idx| SnapshotCell {
                foreground: Rgba::rgb(
                    u8::try_from(idx % 256).unwrap_or(0),
                    u8::try_from((idx * 7) % 256).unwrap_or(0),
                    200,
                ),
                background: Rgba::BLACK,
                flags: GhosttyStyleFlags(u16::try_from(idx % 1024).unwrap_or(0)),
                text: texts[idx % texts.len()].clone(),
            })
            .collect();

        let snapshot = TerminalSnapshot {
            cols,
            rows,
            cursor_x,
            cursor_y,
            cursor_visible,
            cursor_style: GhosttyCursorShape::from_u8(style_idx).unwrap_or(GhosttyCursorShape::Block),
            cursor_blinking,
            alternate_screen,
            foreground: Rgba::WHITE,
            background: Rgba::BLACK,
            cursor_color: Rgba::rgb(0, 159, 255),
            palette,
            cells,
        };

        let encoded = encode_lcvt(&snapshot);
        let decoded = decode_lcvt(&encoded).map_err(|e| TestCaseError::fail(e.to_string()))?;
        prop_assert_eq!(decoded, snapshot);

        if encoded.len() > LCVT_HEADER_BYTES {
            let half = encoded.len() / 2;
            prop_assert!(decode_lcvt(&encoded[..half]).is_err());
        }
    }
}
