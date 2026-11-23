//! Select Graphic Rendition (`SGR`) attribute parsing and color representation.

use serde::{Deserialize, Serialize};

use crate::abi::{
    GhosttyRgb, GhosttySgrUnderline, GhosttyStyle, GhosttyStyleColor, GhosttyStyleFlags,
};

/// Unresolved cell foreground, background, or underline color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CellColor {
    /// Use the terminal's effective default foreground or background color.
    #[default]
    Default,
    /// Indexed color (`0..=255`) resolved through [`crate::palette::AnsiPalette256`].
    Palette(u8),
    /// Direct 24-bit RGB color (`SGR 38;2;R;G;B` / `48;2;R;G;B`).
    Rgb(GhosttyRgb),
}

impl CellColor {
    /// Converts to the Ghostty C ABI tagged style color.
    pub const fn to_ghostty_style_color(self) -> GhosttyStyleColor {
        match self {
            Self::Default => GhosttyStyleColor::none(),
            Self::Palette(idx) => GhosttyStyleColor::palette(idx),
            Self::Rgb(rgb) => GhosttyStyleColor::rgb(rgb),
        }
    }
}

/// Complete active or cell-stored SGR styling attributes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CellAttributes {
    /// Foreground color specification.
    pub fg: CellColor,
    /// Background color specification.
    pub bg: CellColor,
    /// Underline color specification (`SGR 58`).
    pub underline_color: CellColor,
    /// Visual attribute bitflags.
    pub flags: GhosttyStyleFlags,
    /// Underline style variant (`SGR 4` / `4:x`).
    pub underline: GhosttySgrUnderline,
}

impl CellAttributes {
    /// Resets all colors and style decorations to default (`SGR 0`).
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Converts to a sized `GhosttyStyle` C ABI mirror.
    pub const fn to_ghostty_style(&self) -> GhosttyStyle {
        let mut style = GhosttyStyle::new();
        style.fg_color = self.fg.to_ghostty_style_color();
        style.bg_color = self.bg.to_ghostty_style_color();
        style.underline_color = self.underline_color.to_ghostty_style_color();
        style.bold = self.flags.contains(GhosttyStyleFlags::BOLD);
        style.italic = self.flags.contains(GhosttyStyleFlags::ITALIC);
        style.faint = self.flags.contains(GhosttyStyleFlags::FAINT);
        style.blink = self.flags.contains(GhosttyStyleFlags::BLINK);
        style.inverse = self.flags.contains(GhosttyStyleFlags::INVERSE);
        style.invisible = self.flags.contains(GhosttyStyleFlags::INVISIBLE);
        style.strikethrough = self.flags.contains(GhosttyStyleFlags::STRIKETHROUGH);
        style.overline = self.flags.contains(GhosttyStyleFlags::OVERLINE);
        style.underline = self.underline as i32;
        style
    }
}

/// Applies parsed `CSI ... m` parameters (`(value, is_colon_subparam)`) to `attrs`.
pub fn apply_sgr(attrs: &mut CellAttributes, params: &[(u16, bool)]) {
    if params.is_empty() {
        attrs.reset();
        return;
    }
    let mut idx = 0_usize;
    while idx < params.len() {
        let (code, _) = params[idx];
        match code {
            0 => attrs.reset(),
            1 => attrs.flags.insert(GhosttyStyleFlags::BOLD),
            2 => attrs.flags.insert(GhosttyStyleFlags::FAINT),
            3 => attrs.flags.insert(GhosttyStyleFlags::ITALIC),
            4 => idx += apply_underline(attrs, params, idx),
            5 | 6 => attrs.flags.insert(GhosttyStyleFlags::BLINK),
            7 => attrs.flags.insert(GhosttyStyleFlags::INVERSE),
            8 => attrs.flags.insert(GhosttyStyleFlags::INVISIBLE),
            9 => attrs.flags.insert(GhosttyStyleFlags::STRIKETHROUGH),
            21 => {
                attrs.underline = GhosttySgrUnderline::Double;
                attrs.flags.insert(GhosttyStyleFlags::UNDERLINE);
            }
            22 => {
                attrs.flags.remove(GhosttyStyleFlags::BOLD);
                attrs.flags.remove(GhosttyStyleFlags::FAINT);
            }
            23 => attrs.flags.remove(GhosttyStyleFlags::ITALIC),
            24 => {
                attrs.underline = GhosttySgrUnderline::None;
                attrs.flags.remove(GhosttyStyleFlags::UNDERLINE);
            }
            25 => attrs.flags.remove(GhosttyStyleFlags::BLINK),
            27 => attrs.flags.remove(GhosttyStyleFlags::INVERSE),
            28 => attrs.flags.remove(GhosttyStyleFlags::INVISIBLE),
            29 => attrs.flags.remove(GhosttyStyleFlags::STRIKETHROUGH),
            30..=37 => {
                if let Ok(pal) = u8::try_from(code - 30) {
                    attrs.fg = CellColor::Palette(pal);
                }
            }
            38 => idx += apply_extended_color(&mut attrs.fg, params, idx),
            39 => attrs.fg = CellColor::Default,
            40..=47 => {
                if let Ok(pal) = u8::try_from(code - 40) {
                    attrs.bg = CellColor::Palette(pal);
                }
            }
            48 => idx += apply_extended_color(&mut attrs.bg, params, idx),
            49 => attrs.bg = CellColor::Default,
            53 => attrs.flags.insert(GhosttyStyleFlags::OVERLINE),
            55 => attrs.flags.remove(GhosttyStyleFlags::OVERLINE),
            58 => idx += apply_extended_color(&mut attrs.underline_color, params, idx),
            59 => attrs.underline_color = CellColor::Default,
            90..=97 => {
                if let Ok(pal) = u8::try_from(code - 90 + 8) {
                    attrs.fg = CellColor::Palette(pal);
                }
            }
            100..=107 => {
                if let Ok(pal) = u8::try_from(code - 100 + 8) {
                    attrs.bg = CellColor::Palette(pal);
                }
            }
            _ => {}
        }
        idx += 1;
    }
}

fn apply_underline(attrs: &mut CellAttributes, params: &[(u16, bool)], start: usize) -> usize {
    let sub_len = colon_subparam_count(params, start);
    if sub_len == 0 {
        attrs.underline = GhosttySgrUnderline::Single;
        attrs.flags.insert(GhosttyStyleFlags::UNDERLINE);
        return 0;
    }
    let style = match params[start + 1].0 {
        0 => GhosttySgrUnderline::None,
        2 => GhosttySgrUnderline::Double,
        3 => GhosttySgrUnderline::Curly,
        4 => GhosttySgrUnderline::Dotted,
        5 => GhosttySgrUnderline::Dashed,
        _ => GhosttySgrUnderline::Single,
    };
    attrs.underline = style;
    if style == GhosttySgrUnderline::None {
        attrs.flags.remove(GhosttyStyleFlags::UNDERLINE);
    } else {
        attrs.flags.insert(GhosttyStyleFlags::UNDERLINE);
    }
    sub_len
}

fn apply_extended_color(target: &mut CellColor, params: &[(u16, bool)], start: usize) -> usize {
    let sub_len = colon_subparam_count(params, start);
    if sub_len > 0 {
        let sub = &params[start + 1..=start + sub_len];
        match sub.first().map(|entry| entry.0) {
            Some(5) if sub.len() >= 2 => {
                *target = CellColor::Palette(clamp_u8(sub[1].0));
            }
            Some(2) if sub.len() >= 5 => {
                // ITU T.416 `38:2:CS:R:G:B` includes an optional color-space ID before R/G/B.
                *target = CellColor::Rgb(GhosttyRgb::new(
                    clamp_u8(sub[2].0),
                    clamp_u8(sub[3].0),
                    clamp_u8(sub[4].0),
                ));
            }
            Some(2) if sub.len() == 4 => {
                *target = CellColor::Rgb(GhosttyRgb::new(
                    clamp_u8(sub[1].0),
                    clamp_u8(sub[2].0),
                    clamp_u8(sub[3].0),
                ));
            }
            _ => {}
        }
        return sub_len;
    }

    match params.get(start + 1).map(|entry| entry.0) {
        Some(5) => {
            if let Some(&(index, _)) = params.get(start + 2) {
                *target = CellColor::Palette(clamp_u8(index));
                return 2;
            }
            1
        }
        Some(2) => {
            if let (Some(&(r, _)), Some(&(g, _)), Some(&(b, _))) = (
                params.get(start + 2),
                params.get(start + 3),
                params.get(start + 4),
            ) {
                *target = CellColor::Rgb(GhosttyRgb::new(clamp_u8(r), clamp_u8(g), clamp_u8(b)));
                return 4;
            }
            1
        }
        _ => 0,
    }
}

fn colon_subparam_count(params: &[(u16, bool)], start: usize) -> usize {
    params[start + 1..]
        .iter()
        .take_while(|&&(_, is_colon)| is_colon)
        .count()
}

fn clamp_u8(value: u16) -> u8 {
    u8::try_from(value.min(255)).unwrap_or(255)
}
