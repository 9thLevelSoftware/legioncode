//! Tests for `OSC 4` history query sanitization (#40149) and Powerline glyph geometry (#24888).

use t3_terminal_vt::parser_osc::{
    TerminalHistorySanitizer, is_osc_palette_query, sanitize_terminal_history,
    should_strip_osc_sequence,
};
use t3_terminal_vt::powerline::{
    POWERLINE_GLYPHS, PowerlineFlatEdge, PowerlineGlyphKind, PowerlineShape,
    classify_powerline_char, classify_powerline_text, is_powerline_cell_text, is_powerline_char,
};

#[test]
fn osc4_palette_query_detection_and_history_sanitization_match_ghostty_rules() {
    assert!(is_osc_palette_query("4;1;?"));
    assert!(is_osc_palette_query("\n4;1;?;;2;?;3"));
    assert!(!is_osc_palette_query("4;1;red;2;?"));
    assert!(!is_osc_palette_query("4;?"));
    assert!(should_strip_osc_sequence("10;?"));
    assert!(should_strip_osc_sequence("11;rgb:1111/2222/3333"));
    assert!(!should_strip_osc_sequence("0;title"));

    let mut sanitizer = TerminalHistorySanitizer::new();
    let chunk1 = sanitizer.push_chunk(concat!(
        "prompt ",
        "\x1b[!p\x1b[\"p\x1b[4 q\x1b[u",
        "\x1b]4;1;red;2;?\x07",
        "\x1b]0;t",
        "\x1b]\n4;1;?;;2;?;3\x18",
        "\x1b]4;3;?",
    ));
    let chunk2 = sanitizer.push_chunk("\x1b]0;x\x1b\x1b[?2026$p");
    let combined = format!(
        "{}{}{}",
        chunk1.visible_text,
        chunk2.visible_text,
        sanitizer.finish()
    );
    assert_eq!(
        combined,
        concat!(
            "prompt ",
            "\x1b[!p\x1b[\"p\x1b[4 q\x1b[u",
            "\x1b]4;1;red;2;?\x07",
            "\x1b]0;t\x1b\\",
            "\x1b]4;3;?\x1b]0;x\x1b",
        )
    );

    // Standalone terminated BEL and ST OSC 4 queries strip cleanly and emit ST.
    assert_eq!(sanitize_terminal_history("a\x1b]4;0;?\x07b"), "a\x1b\\b");
    assert_eq!(
        sanitize_terminal_history("a\x1b]4;0;?;1;?\x1b\\b"),
        "a\x1b\\b"
    );

    // Split chunk across ESC ] 4 ... \x07 boundary.
    let mut split = TerminalHistorySanitizer::new();
    let p1 = split.push_chunk("hi\x1b]4;2");
    assert_eq!(p1.visible_text, "hi");
    assert_eq!(p1.pending_control_sequence, "\x1b]4;2");
    let p2 = split.push_chunk(";?\x07ok");
    assert_eq!(p2.visible_text, "\x1b\\ok");
    assert_eq!(split.finish(), "");

    // Dynamic colors OSC 10/11/12 still strip completely without emitting ST.
    assert_eq!(
        sanitize_terminal_history("x\x1b]10;?\x07\x1b]11;rgb:0/0/0\x1b\\y"),
        "xy"
    );
}

#[test]
fn powerline_glyphs_cover_e0b0_through_e0bf_with_full_cell_geometry_flags() {
    assert_eq!(POWERLINE_GLYPHS.len(), 16);
    for offset in 0_u32..16 {
        let ch = char::from_u32(0xE0B0 + offset).unwrap_or(' ');
        assert!(is_powerline_char(ch));
        let mut buf = [0_u8; 4];
        let text = ch.encode_utf8(&mut buf);
        assert!(is_powerline_cell_text(text));

        let glyph = classify_powerline_char(ch).unwrap_or(POWERLINE_GLYPHS[0]);
        assert_eq!(glyph.codepoint, ch);
        assert_eq!(classify_powerline_text(text), Some(glyph));
        assert!(glyph.flags.full_cell_geometry);
        assert!(glyph.flags.breaks_text_run);
        assert_eq!(glyph.flags.filled, offset.is_multiple_of(2));
        assert_eq!(glyph.flags.clip_to_cell, !glyph.flags.filled);
        assert_eq!(glyph.flags.needs_seam_overlap, matches!(offset, 4 | 6));
    }

    assert!(!is_powerline_char('\u{E0AF}'));
    assert!(!is_powerline_char('\u{E0C0}'));
    assert!(!is_powerline_cell_text(""));
    assert!(!is_powerline_cell_text("\u{E0B0}\u{E0B0}"));
    assert_eq!(classify_powerline_char('A'), None);

    let right_cap = classify_powerline_char('\u{E0B4}');
    assert!(matches!(
        right_cap.map(|g| (g.kind, g.shape)),
        Some((
            PowerlineGlyphKind::RoundCapRight,
            PowerlineShape::Cap {
                flat_edge: PowerlineFlatEdge::Left,
                filled: true,
            }
        ))
    ));

    let lower_left_slant = classify_powerline_char('\u{E0B8}');
    assert!(matches!(
        lower_left_slant.map(|g| (g.kind, g.flags.filled)),
        Some((PowerlineGlyphKind::LowerLeftSlant, true))
    ));
    let upper_right_stroke = classify_powerline_char('\u{E0BF}');
    assert!(matches!(
        upper_right_stroke.map(|g| (g.kind, g.flags.filled, g.flags.clip_to_cell)),
        Some((PowerlineGlyphKind::UpperRightSlant, false, true))
    ));
}
