//! Canonical xterm 256-color fallback RGB table (`0..=255`).

use crate::abi::GhosttyRgb;

const ANSI_16: [GhosttyRgb; 16] = [
    GhosttyRgb::new(0, 0, 0),
    GhosttyRgb::new(128, 0, 0),
    GhosttyRgb::new(0, 128, 0),
    GhosttyRgb::new(128, 128, 0),
    GhosttyRgb::new(0, 0, 128),
    GhosttyRgb::new(128, 0, 128),
    GhosttyRgb::new(0, 128, 128),
    GhosttyRgb::new(192, 192, 192),
    GhosttyRgb::new(128, 128, 128),
    GhosttyRgb::new(255, 0, 0),
    GhosttyRgb::new(0, 255, 0),
    GhosttyRgb::new(255, 255, 0),
    GhosttyRgb::new(0, 0, 255),
    GhosttyRgb::new(255, 0, 255),
    GhosttyRgb::new(0, 255, 255),
    GhosttyRgb::new(255, 255, 255),
];

const CUBE_LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];

/// Returns the canonical xterm-256 fallback RGB color for palette `index` (`0..=255`).
pub const fn xterm_256_rgb(index: u8) -> GhosttyRgb {
    if index < 16 {
        return ANSI_16[index as usize];
    }
    if index < 232 {
        let cube = index - 16;
        let r = CUBE_LEVELS[(cube / 36) as usize];
        let g = CUBE_LEVELS[((cube / 6) % 6) as usize];
        let b = CUBE_LEVELS[(cube % 6) as usize];
        return GhosttyRgb::new(r, g, b);
    }
    let gray = 8 + 10 * (index - 232);
    GhosttyRgb::new(gray, gray, gray)
}

const fn build_xterm_256_table() -> [GhosttyRgb; 256] {
    let mut table = [GhosttyRgb::new(0, 0, 0); 256];
    let mut i = 0_u8;
    loop {
        table[i as usize] = xterm_256_rgb(i);
        if i == u8::MAX {
            break;
        }
        i += 1;
    }
    table
}

/// Complete compile-time canonical xterm-256 fallback RGB table (`0..=255`).
pub const XTERM_256_PALETTE: [GhosttyRgb; 256] = build_xterm_256_table();
