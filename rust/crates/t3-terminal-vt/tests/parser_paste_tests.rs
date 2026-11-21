//! Integration and property tests for streaming VT/UTF-8 parsing, `TerminalGrid`, and
//! bracketed-paste sanitization.

use proptest::prelude::*;
use t3_terminal_vt::abi::{
    GhosttyCellWide, GhosttyCursorShape, GhosttyMode, GhosttyStyleFlags,
    GhosttyTerminalScrollViewport,
};
use t3_terminal_vt::palette::{Rgba, TerminalThemeConfig};
use t3_terminal_vt::parser::TerminalGrid;
use t3_terminal_vt::paste::{
    encode_paste, is_paste_safe, sanitize_bracketed_paste, wrap_bracketed_paste,
};

#[test]
fn streaming_chunked_utf8_and_vt_matches_single_write() {
    let payload = b"\x1b]2;T3 Shell\x07\x1b[1;3;4:3;38:2::123:45:67;48;5;196me\xcc\x81\xe7\x95\x8c\xf0\x9f\x99\x82\x1b[0m\r\nnext";

    let mut whole = TerminalGrid::new(16, 4);
    whole.feed(payload);

    let mut byte_by_byte = TerminalGrid::new(16, 4);
    for chunk in payload.chunks(1) {
        byte_by_byte.feed(chunk);
    }

    assert_eq!(byte_by_byte.title(), "T3 Shell");
    assert_eq!(byte_by_byte.snapshot(), whole.snapshot());
    assert_eq!(byte_by_byte.viewport_text(), whole.viewport_text());
    assert_eq!(byte_by_byte.cursor_x(), 4);
    assert_eq!(byte_by_byte.cursor_y(), 1);
}

#[test]
fn split_invalid_utf8_recovers_before_escape_sequence() {
    let mut grid = TerminalGrid::new(10, 2);
    // Incomplete 3-byte lead (0xE2 0x82) interrupted by ESC [ 31 m then 'A'.
    grid.feed(b"\xe2\x82");
    grid.feed(b"\x1b[31mA\x1b[0m");
    assert_eq!(grid.cell(0, 0).map(|c| c.text.as_str()), Some("\u{FFFD}"));
    assert_eq!(grid.cell(1, 0).map(|c| c.text.as_str()), Some("A"));
    let snap = grid.snapshot();
    assert_eq!(snap.cells[1].foreground, Rgba::rgb(128, 0, 0));
}

#[test]
fn wide_glyphs_combining_marks_and_faint_inverse_blend_match_ghostty() {
    let mut grid = TerminalGrid::new(12, 3);
    let mut theme = TerminalThemeConfig::new();
    theme.foreground = Some(Rgba::rgb(255, 255, 255));
    theme.background = Some(Rgba::rgb(0, 0, 0));
    grid.apply_theme(&theme);

    grid.feed_str("\x1b[2;7;38;2;40;100;200;48;2;12;34;56mC\x1b[0m界🙂");
    let snap = grid.snapshot();
    assert_eq!(snap.cells[0].text, "C");
    // Inverse swaps fg/bg, then faint blends fg*155 + bg*100 / 255 -> (22, 59, 112).
    assert_eq!(snap.cells[0].foreground, Rgba::rgb(22, 59, 112));
    assert_eq!(snap.cells[0].background, Rgba::rgb(40, 100, 200));
    assert!(snap.cells[0].flags.contains(GhosttyStyleFlags::FAINT));
    assert!(snap.cells[0].flags.contains(GhosttyStyleFlags::INVERSE));

    assert_eq!(grid.cell(1, 0).map(|c| c.wide), Some(GhosttyCellWide::Wide));
    assert_eq!(
        grid.cell(2, 0).map(|c| c.wide),
        Some(GhosttyCellWide::SpacerTail)
    );
    assert_eq!(grid.cell(3, 0).map(|c| c.wide), Some(GhosttyCellWide::Wide));
    assert_eq!(
        grid.cell(4, 0).map(|c| c.wide),
        Some(GhosttyCellWide::SpacerTail)
    );

    // Overwriting the spacer tail clears the leading wide cell cleanly.
    grid.feed_str("\x1b[1;3HX");
    assert_eq!(grid.cell(1, 0).map(|c| c.text.as_str()), Some(""));
    assert_eq!(grid.cell(2, 0).map(|c| c.text.as_str()), Some("X"));
}

#[test]
fn alternate_screen_preserves_primary_buffer_and_cursor() {
    let mut grid = TerminalGrid::new(12, 3);
    grid.feed_str("primary\x1b[2;4H");
    assert_eq!((grid.cursor_x(), grid.cursor_y()), (3, 1));
    assert!(!grid.is_alternate_screen());

    grid.feed_str("\x1b[?1049hALT");
    assert!(grid.is_alternate_screen());
    assert!(grid.mode(GhosttyMode::ALT_SCREEN_SAVE));
    assert_eq!(grid.row_text(0).as_deref(), Some("ALT"));
    assert_eq!(grid.row_text(1).as_deref(), Some(""));

    grid.feed_str("\x1b[?1049l");
    assert!(!grid.is_alternate_screen());
    assert_eq!(grid.row_text(0).as_deref(), Some("primary"));
    assert_eq!((grid.cursor_x(), grid.cursor_y()), (3, 1));
}

#[test]
fn scrollback_erase_margins_and_queries_behave_consistently() {
    let mut grid = TerminalGrid::with_scrollback(10, 3, 5);
    for idx in 0..8 {
        grid.feed_str(&format!("line{idx}\r\n"));
    }
    assert_eq!(grid.scrollback_len(), 5);
    assert_eq!(grid.row_text(0).as_deref(), Some("line6"));
    assert_eq!(grid.row_text(1).as_deref(), Some("line7"));

    grid.scroll_viewport(GhosttyTerminalScrollViewport::top());
    assert_eq!(grid.scroll_offset_from_bottom(), 5);
    assert!(!grid.snapshot().cursor_visible);
    assert_eq!(grid.snapshot().cursor_x, u16::MAX);

    grid.scroll_viewport(GhosttyTerminalScrollViewport::bottom());
    assert_eq!(grid.scroll_offset_from_bottom(), 0);
    assert!(grid.snapshot().cursor_visible);

    // DECSTBM scroll region + DSR + DECSCUSR + OSC ST.
    grid.feed_str("\x1b[1;2r\x1b[1;1Htop\r\nmid\r\nbot\x1b[5 q\x1b[5n\x1b[6n\x1b]0;ST Title\x1b\\");
    assert_eq!(grid.title(), "ST Title");
    assert_eq!(grid.cursor_shape(), GhosttyCursorShape::Bar);
    assert!(grid.cursor_blinking());
    assert_eq!(grid.drain_responses(), b"\x1b[0n\x1b[2;4R");

    // ED 3 clears screen + scrollback.
    grid.feed_str("\x1b[3J");
    assert_eq!(grid.scrollback_len(), 0);
}

#[test]
fn osc_dynamic_colors_resize_and_viewport_bounds_behave_cleanly() {
    assert!(TerminalGrid::try_new(0, 10).is_err());
    let mut grid = TerminalGrid::new(6, 3);
    grid.feed_str("\x1b]0;a\x00b\x07\x1b]10;#112233\x07\x1b]11;?\x1b\\");
    assert_eq!(grid.title(), "ab");
    assert_eq!(grid.icon_title(), "ab");
    assert_eq!(grid.effective_foreground(), Rgba::rgb(0x11, 0x22, 0x33));
    assert!(
        String::from_utf8_lossy(&grid.drain_responses())
            .contains("\x1b]11;rgb:2424/2929/2e2e\x1b\\")
    );
    grid.feed_str("\x1b]110\x07");
    assert_eq!(grid.effective_foreground(), Rgba::rgb(0xD1, 0xD5, 0xDA));

    // Resizing narrower splits trailing wide glyph cleanly instead of leaving orphan SpacerTail.
    grid.feed_str("\x1b[1;1H界界");
    grid.resize(3, 2);
    assert_eq!(grid.cols(), 3);
    assert_eq!(grid.rows(), 2);
    assert_eq!(grid.cell(2, 0).map(|c| c.text.as_str()), Some(""));
    assert_eq!(grid.viewport().cols, 3);
    assert_eq!(grid.viewport().rows, 2);
}

#[test]
fn bracketed_paste_sanitizer_neutralizes_escapes_and_preserves_safe_whitespace() {
    assert_eq!(
        sanitize_bracketed_paste("hello\tworld\nline2"),
        "hello\tworld\nline2"
    );
    assert_eq!(
        sanitize_bracketed_paste("one\r\ntwo\rthree"),
        "one\ntwothree"
    );
    assert_eq!(
        sanitize_bracketed_paste("safe\x1b[201~; rm -rf /\n"),
        "safe; rm -rf /\n"
    );
    assert_eq!(sanitize_bracketed_paste("a\x1b[0201~b\x1b[200~c"), "abc");
    assert_eq!(sanitize_bracketed_paste("a\u{009B}201~b"), "ab");
    assert_eq!(sanitize_bracketed_paste("a\x1b]52;c;dGVzdA==\x07b"), "ab");
    assert_eq!(sanitize_bracketed_paste("a\x1b]0;evil\x1b\\b"), "ab");
    assert_eq!(sanitize_bracketed_paste("a\x1b]0;unterminated\nb"), "a\nb");
    assert_eq!(sanitize_bracketed_paste("a\x00\x01\x08\x7f\u{85}b"), "ab");
    // Split/nested marker reconstruction attempt.
    assert_eq!(sanitize_bracketed_paste("x\x1b[20\x1b[201~1~y"), "xy");
    assert_eq!(sanitize_bracketed_paste("x\x1b[\x00201~y"), "xy");

    assert_eq!(
        wrap_bracketed_paste("a\x1b[201~b\nc"),
        "\x1b[200~ab\nc\x1b[201~"
    );
    assert_eq!(encode_paste("one\r\ntwo\x1b[201~!", false), "one\rtwo!");
    assert!(!is_paste_safe("echo hi\n"));
    assert!(!is_paste_safe("a\x1b[201~b"));
    assert!(is_paste_safe("safe\ttext 123"));
}

proptest! {
    #[test]
    fn prop_vt_parser_never_panics_and_keeps_bounds(
        chunks in proptest::collection::vec(proptest::collection::vec(any::<u8>(), 0..64), 0..24),
        cols in 1_u16..40,
        rows in 1_u16..16,
    ) {
        let mut grid = TerminalGrid::with_scrollback(cols, rows, 32);
        let mut combined = Vec::new();
        for chunk in &chunks {
            grid.feed(chunk);
            combined.extend_from_slice(chunk);
            prop_assert!(grid.cursor_x() < cols);
            prop_assert!(grid.cursor_y() < rows);
            prop_assert!(grid.scrollback_len() <= 32);
        }
        let mut reference = TerminalGrid::with_scrollback(cols, rows, 32);
        reference.feed(&combined);
        prop_assert_eq!(grid.snapshot(), reference.snapshot());
    }

    #[test]
    fn prop_bracketed_paste_never_emits_escape_or_unsafe_controls(
        parts in proptest::collection::vec(
            prop_oneof![
                any::<String>(),
                Just("\x1b[201~".to_owned()),
                Just("\x1b[200~".to_owned()),
                Just("\u{009B}201~".to_owned()),
                Just("\x1b]52;c;evil\x07".to_owned()),
                Just("\x1b]0;title\x1b\\".to_owned()),
                Just("\x00\x07\x08\r\x7f\u{009D}".to_owned()),
                Just("\n\t".to_owned()),
            ],
            0..16,
        )
    ) {
        let raw = parts.concat();
        let cleaned = sanitize_bracketed_paste(&raw);
        prop_assert!(!cleaned.contains('\x1b'));
        prop_assert!(!cleaned.contains("\x1b[201~"));
        prop_assert!(!cleaned.contains("\x1b[200~"));
        prop_assert!(!cleaned.contains('\x07'));
        prop_assert!(!cleaned.contains('\r'));
        prop_assert!(!cleaned.contains('\x7f'));
        let has_unsafe = cleaned.chars().any(|ch| {
            matches!(ch, '\x00'..='\x1f' | '\u{0080}'..='\u{009f}') && ch != '\n' && ch != '\t'
        });
        prop_assert!(!has_unsafe);
    }
}
