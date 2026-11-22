//! Ghostty theme color parsing (`#RGB`, `#RRGGBB`, `#RRGGBBAA`, `rgb:R/G/B`) and theme config
//! extraction matching `ghostty/vt/color.h` and `LegionTerminalView.kt`.

use crate::error::VtError;
use crate::palette::{AnsiPalette256, Rgba};

/// Parsed terminal theme defaults and sparse 256-color palette overrides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalThemeConfig {
    /// Default foreground color when specified in config.
    pub foreground: Option<Rgba>,
    /// Default background color when specified in config.
    pub background: Option<Rgba>,
    /// Default cursor color when specified in config.
    pub cursor_color: Option<Rgba>,
    /// Default cursor text color when specified in config.
    pub cursor_text: Option<Rgba>,
    /// Sparse 256-color ANSI palette (`BUG-25`: unspecified indices stay `None`).
    pub palette: AnsiPalette256,
}

impl Default for TerminalThemeConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl TerminalThemeConfig {
    /// Creates an empty theme config with no color or palette overrides.
    pub fn new() -> Self {
        Self {
            foreground: None,
            background: None,
            cursor_color: None,
            cursor_text: None,
            palette: AnsiPalette256::new(),
        }
    }

    /// Parses a Ghostty-style `key = value` theme configuration string (`BUG-25`).
    ///
    /// Empty or palette-free configs produce an all-`None` [`AnsiPalette256`], ensuring prior
    /// theme overrides never leak across theme changes and sparse entries never fill gaps with
    /// `foreground`.
    pub fn parse(config: &str) -> Self {
        let mut parsed = Self::new();
        let mut overrides = Vec::new();
        for line in config.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let Some((raw_key, raw_value)) = trimmed.split_once('=') else {
                continue;
            };
            let key = raw_key.trim();
            let value = raw_value.trim();
            match key {
                "foreground" => {
                    if let Ok(color) = parse_color(value) {
                        parsed.foreground = Some(color);
                    }
                }
                "background" => {
                    if let Ok(color) = parse_color(value) {
                        parsed.background = Some(color);
                    }
                }
                "cursor-color" => {
                    if let Ok(color) = parse_color(value) {
                        parsed.cursor_color = Some(color);
                    }
                }
                "cursor-text" => {
                    if let Ok(color) = parse_color(value) {
                        parsed.cursor_text = Some(color);
                    }
                }
                "palette" => {
                    if let Some((index, color)) = parse_palette_entry(value) {
                        overrides.push((index, color));
                    }
                }
                _ => {}
            }
        }
        parsed.palette.apply_theme_overrides(&overrides);
        parsed
    }
}

/// Parses a Ghostty palette line value (`INDEX=COLOR`), accepting decimal, `0x`, `0o`, or `0b`
/// indices in `0..=255`.
pub fn parse_palette_entry(value: &str) -> Option<(u8, Rgba)> {
    let (raw_index, raw_color) = value.split_once('=')?;
    let index = parse_palette_index(raw_index.trim())?;
    let color = parse_color(raw_color.trim()).ok()?;
    Some((index, color))
}

fn parse_palette_index(raw: &str) -> Option<u8> {
    if let Some(hex) = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
        u8::from_str_radix(hex, 16).ok()
    } else if let Some(oct) = raw.strip_prefix("0o").or_else(|| raw.strip_prefix("0O")) {
        u8::from_str_radix(oct, 8).ok()
    } else if let Some(bin) = raw.strip_prefix("0b").or_else(|| raw.strip_prefix("0B")) {
        u8::from_str_radix(bin, 2).ok()
    } else {
        raw.parse::<u8>().ok()
    }
}

/// Parses `#RGB`, `#RRGGBB`, `#RRGGBBAA`, bare `RGB`/`RRGGBB`, or `rgb:R/G/B` color syntax.
pub fn parse_color(input: &str) -> Result<Rgba, VtError> {
    let trimmed = input.trim();
    if let Some(body) = trimmed
        .strip_prefix("rgb:")
        .or_else(|| trimmed.strip_prefix("RGB:"))
    {
        return parse_x11_rgb_slash(body, trimmed);
    }
    let hex = trimmed.strip_prefix('#').unwrap_or(trimmed);
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(VtError::InvalidColor(trimmed.to_owned()));
    }
    match hex.len() {
        3 => {
            let r = parse_hex_nibble(hex, 0, trimmed)?;
            let g = parse_hex_nibble(hex, 1, trimmed)?;
            let b = parse_hex_nibble(hex, 2, trimmed)?;
            Ok(Rgba::rgb(r * 17, g * 17, b * 17))
        }
        6 => {
            let r = parse_hex_byte(hex, 0..2, trimmed)?;
            let g = parse_hex_byte(hex, 2..4, trimmed)?;
            let b = parse_hex_byte(hex, 4..6, trimmed)?;
            Ok(Rgba::rgb(r, g, b))
        }
        8 => {
            let r = parse_hex_byte(hex, 0..2, trimmed)?;
            let g = parse_hex_byte(hex, 2..4, trimmed)?;
            let b = parse_hex_byte(hex, 4..6, trimmed)?;
            let a = parse_hex_byte(hex, 6..8, trimmed)?;
            Ok(Rgba::new(r, g, b, a))
        }
        _ => Err(VtError::InvalidColor(trimmed.to_owned())),
    }
}

fn parse_x11_rgb_slash(body: &str, original: &str) -> Result<Rgba, VtError> {
    let mut parts = body.split('/');
    let Some(r_raw) = parts.next() else {
        return Err(VtError::InvalidColor(original.to_owned()));
    };
    let Some(g_raw) = parts.next() else {
        return Err(VtError::InvalidColor(original.to_owned()));
    };
    let Some(b_raw) = parts.next() else {
        return Err(VtError::InvalidColor(original.to_owned()));
    };
    if parts.next().is_some() {
        return Err(VtError::InvalidColor(original.to_owned()));
    }
    Ok(Rgba::rgb(
        scale_x11_component(r_raw, original)?,
        scale_x11_component(g_raw, original)?,
        scale_x11_component(b_raw, original)?,
    ))
}

fn scale_x11_component(component: &str, original: &str) -> Result<u8, VtError> {
    if component.is_empty()
        || component.len() > 4
        || !component.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(VtError::InvalidColor(original.to_owned()));
    }
    let val = u32::from_str_radix(component, 16)
        .map_err(|_| VtError::InvalidColor(original.to_owned()))?;
    let max_val = match component.len() {
        1 => 0xF_u32,
        2 => 0xFF_u32,
        3 => 0xFFF_u32,
        _ => 0xFFFF_u32,
    };
    let scaled = (val * 255 + (max_val / 2)) / max_val;
    u8::try_from(scaled).map_err(|_| VtError::InvalidColor(original.to_owned()))
}

fn parse_hex_nibble(hex: &str, index: usize, original: &str) -> Result<u8, VtError> {
    let slice = hex
        .get(index..=index)
        .ok_or_else(|| VtError::InvalidColor(original.to_owned()))?;
    u8::from_str_radix(slice, 16).map_err(|_| VtError::InvalidColor(original.to_owned()))
}

fn parse_hex_byte(hex: &str, range: std::ops::Range<usize>, original: &str) -> Result<u8, VtError> {
    let slice = hex
        .get(range)
        .ok_or_else(|| VtError::InvalidColor(original.to_owned()))?;
    u8::from_str_radix(slice, 16).map_err(|_| VtError::InvalidColor(original.to_owned()))
}
