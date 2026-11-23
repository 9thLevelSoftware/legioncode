//! Integration and property tests for `BUG-25` sparse 256-color palette and Ghostty VT ABI
//! layout mirrors.

use std::mem::{align_of, offset_of, size_of};

use legion_terminal_vt::abi::{
    GhosttyCell, GhosttyCellContentTag, GhosttyCellWide, GhosttyColorPaletteMask,
    GhosttyCursorShape, GhosttyMode, GhosttyRenderStateColors, GhosttyRow, GhosttyStyle,
    GhosttyStyleColor, GhosttyStyleColorTag, GhosttyStyleColorValue, GhosttyStyleFlags,
    GhosttyTerminalOptions, GhosttyTerminalScrollViewport, GhosttyTerminalScrollbar,
    GhosttyViewport,
};
use legion_terminal_vt::palette::{
    AnsiPalette256, Rgba, TerminalThemeConfig, XTERM_256_PALETTE, parse_color, xterm_256_rgb,
};
use proptest::prelude::*;

#[test]
fn bug25_empty_theme_palette_clears_all_prior_overrides() {
    let mut palette = AnsiPalette256::new();
    let red = Rgba::rgb(0xFF, 0x2E, 0x3F);
    let blue = Rgba::rgb(0x00, 0x9F, 0xFF);
    palette.apply_theme_overrides(&[(1, red), (4, blue), (200, Rgba::rgb(10, 20, 30))]);
    assert_eq!(palette.override_at(1), Some(red));
    assert_eq!(palette.override_at(4), Some(blue));
    assert_eq!(palette.override_at(200), Some(Rgba::rgb(10, 20, 30)));

    // Applying an empty theme palette MUST reset all 256 slots to None.
    palette.apply_theme_overrides(&[]);
    for idx in 0_u8..=255 {
        assert_eq!(palette.override_at(idx), None);
        assert_eq!(palette.resolve_rgb(idx), xterm_256_rgb(idx));
    }
}

#[test]
fn bug25_sparse_overrides_never_fill_unset_slots_with_foreground() {
    let fg = Rgba::rgb(0xD1, 0xD5, 0xDA);
    let config = "\
foreground = #d1d5da
background = #24292e
cursor-color = #009fff
palette = 1=#ff2e3f
palette = 15=#c6c6c8
palette = 196=#ff1122
";
    let parsed = TerminalThemeConfig::parse(config);
    assert_eq!(parsed.foreground, Some(fg));
    assert_eq!(parsed.palette.override_at(0), None);
    assert_eq!(
        parsed.palette.override_at(1),
        Some(Rgba::rgb(0xFF, 0x2E, 0x3F))
    );
    assert_eq!(parsed.palette.override_at(2), None);
    assert_eq!(parsed.palette.override_at(14), None);
    assert_eq!(
        parsed.palette.override_at(15),
        Some(Rgba::rgb(0xC6, 0xC6, 0xC8))
    );
    assert_eq!(parsed.palette.override_at(195), None);
    assert_eq!(
        parsed.palette.override_at(196),
        Some(Rgba::rgb(0xFF, 0x11, 0x22))
    );
    assert_eq!(parsed.palette.override_at(255), None);

    // Every unspecified slot resolves via xterm-256 fallback, NEVER foreground_color.
    for idx in [0_u8, 2, 7, 14, 16, 21, 100, 195, 231, 232, 255] {
        assert_eq!(parsed.palette.override_at(idx), None);
        assert_ne!(parsed.palette.resolve(idx), fg);
        assert_eq!(
            parsed.palette.resolve_rgb(idx),
            XTERM_256_PALETTE[usize::from(idx)]
        );
    }

    // Switching to a theme config with no `palette =` entries clears old overrides.
    let mut active = parsed.palette;
    active.apply_theme_config("foreground = #ffffff\nbackground = #000000\n");
    for idx in 0_u8..=255 {
        assert_eq!(active.override_at(idx), None);
    }
}

#[test]
fn xterm256_canonical_cube_and_grayscale_values_match() {
    assert_eq!(xterm_256_rgb(0), (0, 0, 0).into_rgb());
    assert_eq!(xterm_256_rgb(15), (255, 255, 255).into_rgb());
    assert_eq!(xterm_256_rgb(16), (0, 0, 0).into_rgb());
    assert_eq!(xterm_256_rgb(21), (0, 0, 255).into_rgb());
    assert_eq!(xterm_256_rgb(196), (255, 0, 0).into_rgb());
    assert_eq!(xterm_256_rgb(231), (255, 255, 255).into_rgb());
    assert_eq!(xterm_256_rgb(232), (8, 8, 8).into_rgb());
    assert_eq!(xterm_256_rgb(255), (238, 238, 238).into_rgb());
}

trait IntoRgb {
    fn into_rgb(self) -> legion_terminal_vt::abi::GhosttyRgb;
}

impl IntoRgb for (u8, u8, u8) {
    fn into_rgb(self) -> legion_terminal_vt::abi::GhosttyRgb {
        legion_terminal_vt::abi::GhosttyRgb::new(self.0, self.1, self.2)
    }
}

#[test]
fn ghostty_color_parsing_supports_hex_and_x11_rgb_syntax() -> Result<(), String> {
    assert_eq!(
        parse_color("#abc").map_err(|e| e.to_string())?,
        Rgba::rgb(0xAA, 0xBB, 0xCC)
    );
    assert_eq!(
        parse_color("24292e").map_err(|e| e.to_string())?,
        Rgba::rgb(0x24, 0x29, 0x2E)
    );
    assert_eq!(
        parse_color("#11223380").map_err(|e| e.to_string())?,
        Rgba::new(0x11, 0x22, 0x33, 0x80)
    );
    assert_eq!(
        parse_color("rgb:11/22/33").map_err(|e| e.to_string())?,
        Rgba::rgb(0x11, 0x22, 0x33)
    );
    assert_eq!(
        parse_color("rgb:ffff/0000/8080").map_err(|e| e.to_string())?,
        Rgba::rgb(0xFF, 0x00, 0x80)
    );
    assert!(parse_color("not-a-color").is_err());
    Ok(())
}

#[test]
fn abi_mirror_layouts_match_ghostty_vt_headers() {
    assert_eq!(size_of::<legion_terminal_vt::abi::GhosttyRgb>(), 3);
    assert_eq!(align_of::<legion_terminal_vt::abi::GhosttyRgb>(), 1);
    assert_eq!(size_of::<GhosttyColorPaletteMask>(), 32);
    assert_eq!(align_of::<GhosttyColorPaletteMask>(), 8);
    assert_eq!(size_of::<GhosttyStyleColorValue>(), 8);
    assert_eq!(align_of::<GhosttyStyleColorValue>(), 8);
    assert_eq!(size_of::<GhosttyStyleColor>(), 16);
    assert_eq!(align_of::<GhosttyStyleColor>(), 8);
    assert_eq!(offset_of!(GhosttyStyleColor, tag), 0);
    assert_eq!(offset_of!(GhosttyStyleColor, value), 8);

    assert_eq!(size_of::<GhosttyStyle>(), 72);
    assert_eq!(align_of::<GhosttyStyle>(), 8);
    assert_eq!(offset_of!(GhosttyStyle, size), 0);
    assert_eq!(offset_of!(GhosttyStyle, fg_color), 8);
    assert_eq!(offset_of!(GhosttyStyle, bg_color), 24);
    assert_eq!(offset_of!(GhosttyStyle, underline_color), 40);
    assert_eq!(offset_of!(GhosttyStyle, bold), 56);
    assert_eq!(offset_of!(GhosttyStyle, overline), 63);
    assert_eq!(offset_of!(GhosttyStyle, underline), 64);

    assert_eq!(size_of::<GhosttyTerminalScrollViewport>(), 24);
    assert_eq!(align_of::<GhosttyTerminalScrollViewport>(), 8);
    assert_eq!(size_of::<GhosttyTerminalScrollbar>(), 24);
    assert_eq!(align_of::<GhosttyTerminalScrollbar>(), 8);
    assert_eq!(size_of::<GhosttyCell>(), 8);
    assert_eq!(size_of::<GhosttyRow>(), 8);
    assert_eq!(size_of::<GhosttyStyleFlags>(), 2);
    assert_eq!(
        size_of::<GhosttyTerminalOptions>(),
        if cfg!(target_pointer_width = "32") {
            8
        } else {
            16
        }
    );
    assert_eq!(
        size_of::<GhosttyRenderStateColors>(),
        if cfg!(target_pointer_width = "32") {
            784
        } else {
            792
        }
    );
    let _viewport = GhosttyViewport::default();
}

#[test]
fn abi_bitfields_and_tagged_accessors_round_trip() {
    let mut style = GhosttyStyle::new();
    assert_eq!(style.size, size_of::<GhosttyStyle>());
    style.fg_color = GhosttyStyleColor::palette(196);
    style.bg_color = GhosttyStyleColor::rgb(legion_terminal_vt::abi::GhosttyRgb::new(10, 20, 30));
    style.bold = true;
    style.inverse = true;
    style.underline = 3;
    assert_eq!(style.fg_color.tag, GhosttyStyleColorTag::Palette);
    assert_eq!(style.fg_color.value.as_palette(), 196);
    assert_eq!(style.bg_color.tag, GhosttyStyleColorTag::Rgb);
    assert_eq!(
        style.bg_color.value.as_rgb(),
        legion_terminal_vt::abi::GhosttyRgb::new(10, 20, 30)
    );

    let flags = GhosttyStyleFlags::from_style(&style, true);
    assert!(flags.contains(GhosttyStyleFlags::BOLD));
    assert!(flags.contains(GhosttyStyleFlags::INVERSE));
    assert!(flags.contains(GhosttyStyleFlags::UNDERLINE));
    assert!(flags.contains(GhosttyStyleFlags::SELECTED));
    assert!(!flags.contains(GhosttyStyleFlags::ITALIC));
    assert_eq!(
        GhosttyCursorShape::from_u8(2),
        Some(GhosttyCursorShape::Underline)
    );
    assert_eq!(GhosttyCursorShape::from_u8(9), None);

    let cell = GhosttyCell::from_codepoint('界' as u32, 42, GhosttyCellWide::Wide, true, false);
    assert_eq!(cell.content_tag(), GhosttyCellContentTag::Codepoint);
    assert_eq!(cell.codepoint(), '界' as u32);
    assert!(cell.has_text());
    assert_eq!(cell.style_id(), 42);
    assert!(cell.has_styling());
    assert_eq!(cell.wide(), GhosttyCellWide::Wide);
    assert!(cell.has_hyperlink());

    let row = GhosttyRow::from_flags(true, false, true);
    assert!(row.wrap());
    assert!(!row.wrap_continuation());
    assert!(row.dirty());

    assert_eq!(GhosttyMode::BRACKETED_PASTE.value(), 2004);
    assert!(!GhosttyMode::BRACKETED_PASTE.is_ansi());
    assert_eq!(GhosttyMode::INSERT.value(), 4);
    assert!(GhosttyMode::INSERT.is_ansi());

    let mut mask = GhosttyColorPaletteMask::default();
    mask.set(200);
    assert!(mask.is_set(200));
    mask.unset(200);
    assert!(!mask.is_set(200));
}

proptest! {
    #[test]
    fn prop_bug25_sparse_overrides_isolate_unset_slots_and_reset_cleanly(
        first in proptest::collection::vec((any::<u8>(), any::<u8>(), any::<u8>(), any::<u8>()), 0..64),
        second in proptest::collection::vec((any::<u8>(), any::<u8>(), any::<u8>(), any::<u8>()), 0..32),
    ) {
        let mut palette = AnsiPalette256::new();
        let first_pairs: Vec<(u8, Rgba)> = first.iter().map(|&(i, r, g, b)| (i, Rgba::rgb(r, g, b))).collect();
        palette.apply_theme_overrides(&first_pairs);

        let second_pairs: Vec<(u8, Rgba)> = second.iter().map(|&(i, r, g, b)| (i, Rgba::rgb(r, g, b))).collect();
        palette.apply_theme_overrides(&second_pairs);

        for idx in 0_u8..=255 {
            let expected = second_pairs.iter().rev().find(|&&(i, _)| i == idx).map(|&(_, c)| c);
            prop_assert_eq!(palette.override_at(idx), expected);
            if expected.is_none() {
                prop_assert_eq!(palette.resolve_rgb(idx), xterm_256_rgb(idx));
            }
        }

        palette.apply_theme_overrides(&[]);
        for idx in 0_u8..=255 {
            prop_assert_eq!(palette.override_at(idx), None);
        }
    }
}
