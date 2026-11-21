//! Terminal, point, render-state, mode, and viewport ABI mirrors for `libghostty-vt`.

use serde::{Deserialize, Serialize};

use crate::abi::{GhosttyCursorShape, GhosttyRgb};

/// Packed 16-bit terminal mode identifier (`ghostty/vt/modes.h`).
///
/// Bits `0..=14` store the numeric mode number and bit `15` is `1` for ANSI modes
/// or `0` for DEC private (`?`-prefixed) modes.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct GhosttyMode(pub u16);

impl GhosttyMode {
    /// Cursor keys mode (DECCKM, `?1`).
    pub const DECCKM: Self = Self::dec(1);
    /// Origin mode (DECOM, `?6`).
    pub const ORIGIN: Self = Self::dec(6);
    /// Auto-wrap mode (DECAWM, `?7`).
    pub const WRAPAROUND: Self = Self::dec(7);
    /// Cursor blink (DEC `?12`).
    pub const CURSOR_BLINKING: Self = Self::dec(12);
    /// Cursor visible (DECTCEM, `?25`).
    pub const CURSOR_VISIBLE: Self = Self::dec(25);
    /// Legacy alternate screen (DEC `?47`).
    pub const ALT_SCREEN_LEGACY: Self = Self::dec(47);
    /// Alternate screen (DEC `?1047`).
    pub const ALT_SCREEN: Self = Self::dec(1047);
    /// Save cursor + alternate screen + clear (DEC `?1049`).
    pub const ALT_SCREEN_SAVE: Self = Self::dec(1049);
    /// Bracketed paste mode (DEC `?2004`).
    pub const BRACKETED_PASTE: Self = Self::dec(2004);
    /// Synchronized output mode (DEC `?2026`).
    pub const SYNC_OUTPUT: Self = Self::dec(2026);
    /// Insert mode (IRM, ANSI `4`).
    pub const INSERT: Self = Self::ansi(4);
    /// Linefeed/new-line mode (LNM, ANSI `20`).
    pub const LINEFEED: Self = Self::ansi(20);

    /// Creates a mode value matching `ghostty_mode_new(value, ansi)`.
    pub const fn new(value: u16, ansi: bool) -> Self {
        Self((value & 0x7FFF) | ((ansi as u16) << 15))
    }

    /// Creates a DEC private mode (`?`-prefixed).
    pub const fn dec(value: u16) -> Self {
        Self::new(value, false)
    }

    /// Creates a standard ANSI mode.
    pub const fn ansi(value: u16) -> Self {
        Self::new(value, true)
    }

    /// Extracts the 15-bit mode number (`ghostty_mode_value`).
    pub const fn value(self) -> u16 {
        self.0 & 0x7FFF
    }

    /// Returns `true` when this mode is an ANSI mode (`ghostty_mode_ansi`).
    pub const fn is_ansi(self) -> bool {
        (self.0 >> 15) != 0
    }
}

/// Terminal initialization options (`GhosttyTerminalOptions` in `ghostty/vt/terminal.h`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GhosttyTerminalOptions {
    /// Terminal width in cells.
    pub cols: u16,
    /// Terminal height in cells.
    pub rows: u16,
    /// Scrollback limit (`size_t`).
    pub max_scrollback: usize,
}

/// Active terminal screen identifier (`GhosttyTerminalScreen` in `ghostty/vt/terminal.h`).
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum GhosttyTerminalScreen {
    /// Primary (normal) screen buffer.
    #[default]
    Primary = 0,
    /// Alternate screen buffer.
    Alternate = 1,
}

/// Scroll viewport behavior tag (`GhosttyTerminalScrollViewportTag`).
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GhosttyTerminalScrollViewportTag {
    /// Scroll to the top of the scrollback buffer.
    #[default]
    Top = 0,
    /// Scroll to the bottom (active area).
    Bottom = 1,
    /// Scroll by a signed row delta (negative is up).
    Delta = 2,
    /// Scroll to an absolute row offset from the top of the scrollable area.
    Row = 3,
}

/// Scroll viewport value union mirror (`GhosttyTerminalScrollViewportValue`, 16 bytes, align 8).
#[repr(C, align(8))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GhosttyTerminalScrollViewportValue {
    bytes: [u8; 16],
}

impl GhosttyTerminalScrollViewportValue {
    /// Constructs a signed delta payload (`intptr_t`) at offset `0`.
    pub fn from_delta(delta: isize) -> Self {
        let mut out = Self::default();
        let raw = delta.to_ne_bytes();
        out.bytes[..raw.len()].copy_from_slice(&raw);
        out
    }

    /// Reads the signed delta payload (`intptr_t`).
    pub fn as_delta(self) -> isize {
        let width = std::mem::size_of::<isize>();
        let mut raw = [0_u8; std::mem::size_of::<isize>()];
        raw.copy_from_slice(&self.bytes[..width]);
        isize::from_ne_bytes(raw)
    }

    /// Constructs an absolute row offset payload (`size_t`) at offset `0`.
    pub fn from_row(row: usize) -> Self {
        let mut out = Self::default();
        let raw = row.to_ne_bytes();
        out.bytes[..raw.len()].copy_from_slice(&raw);
        out
    }

    /// Reads the absolute row offset payload (`size_t`).
    pub fn as_row(self) -> usize {
        let width = std::mem::size_of::<usize>();
        let mut raw = [0_u8; std::mem::size_of::<usize>()];
        raw.copy_from_slice(&self.bytes[..width]);
        usize::from_ne_bytes(raw)
    }
}

/// Tagged union for viewport scrolling (`GhosttyTerminalScrollViewport`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GhosttyTerminalScrollViewport {
    /// Scroll behavior variant.
    pub tag: GhosttyTerminalScrollViewportTag,
    /// Union payload.
    pub value: GhosttyTerminalScrollViewportValue,
}

impl GhosttyTerminalScrollViewport {
    /// Scroll to the top of scrollback.
    pub fn top() -> Self {
        Self {
            tag: GhosttyTerminalScrollViewportTag::Top,
            value: GhosttyTerminalScrollViewportValue::default(),
        }
    }

    /// Scroll to the bottom (active screen).
    pub fn bottom() -> Self {
        Self {
            tag: GhosttyTerminalScrollViewportTag::Bottom,
            value: GhosttyTerminalScrollViewportValue::default(),
        }
    }

    /// Scroll by a relative row delta (negative up, positive down).
    pub fn delta(delta: isize) -> Self {
        Self {
            tag: GhosttyTerminalScrollViewportTag::Delta,
            value: GhosttyTerminalScrollViewportValue::from_delta(delta),
        }
    }

    /// Scroll to an absolute row offset from the top of the scrollable area.
    pub fn row(row: usize) -> Self {
        Self {
            tag: GhosttyTerminalScrollViewportTag::Row,
            value: GhosttyTerminalScrollViewportValue::from_row(row),
        }
    }
}

/// Scrollbar state for the terminal viewport (`GhosttyTerminalScrollbar`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct GhosttyTerminalScrollbar {
    /// Total scrollable area size in rows.
    pub total: u64,
    /// Offset of the top visible viewport row.
    pub offset: u64,
    /// Visible viewport height in rows.
    pub len: u64,
}

/// Render-state dirty level (`GhosttyRenderStateDirty` in `ghostty/vt/render.h`).
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GhosttyRenderStateDirty {
    /// Nothing changed.
    #[default]
    False = 0,
    /// Some rows changed.
    Partial = 1,
    /// Global state changed; redraw everything.
    Full = 2,
}

/// Sized render-state color block (`GhosttyRenderStateColors` in `ghostty/vt/render.h`).
#[repr(C)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GhosttyRenderStateColors {
    /// `sizeof(GhosttyRenderStateColors)`.
    pub size: usize,
    /// Default/current background color.
    pub background: GhosttyRgb,
    /// Default/current foreground color.
    pub foreground: GhosttyRgb,
    /// Explicit cursor color when `cursor_has_value` is `true`.
    pub cursor: GhosttyRgb,
    /// Whether `cursor` holds an explicit cursor color.
    pub cursor_has_value: bool,
    /// Active 256-color palette.
    pub palette: [GhosttyRgb; 256],
}

impl GhosttyRenderStateColors {
    /// Creates a sized `GhosttyRenderStateColors` initialized with `size_of::<Self>()`.
    pub fn new(
        background: GhosttyRgb,
        foreground: GhosttyRgb,
        cursor: Option<GhosttyRgb>,
        palette: [GhosttyRgb; 256],
    ) -> Self {
        Self {
            size: std::mem::size_of::<Self>(),
            background,
            foreground,
            cursor: cursor.unwrap_or(foreground),
            cursor_has_value: cursor.is_some(),
            palette,
        }
    }
}

/// C-ABI viewport summary capturing visible dimensions, scroll state, and cursor geometry.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct GhosttyViewport {
    /// Viewport width in columns.
    pub cols: u16,
    /// Viewport height in rows.
    pub rows: u16,
    /// Top row offset from the top of the scrollable area (`0` when at top).
    pub scroll_offset: u64,
    /// Total rows including scrollback and active screen.
    pub total_rows: u64,
    /// Viewport cursor column (`0`-indexed, `0xFFFF` if outside viewport).
    pub cursor_x: u16,
    /// Viewport cursor row (`0`-indexed, `0xFFFF` if outside viewport).
    pub cursor_y: u16,
    /// Visual cursor shape.
    pub cursor_shape: GhosttyCursorShape,
    /// Whether cursor is visible within the viewport.
    pub cursor_visible: bool,
    /// Whether cursor blinking is enabled.
    pub cursor_blinking: bool,
    /// Whether alternate screen buffer is currently active.
    pub alternate_screen: bool,
}

/// Terminal point coordinate (`GhosttyPointCoordinate` in `ghostty/vt/point.h`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GhosttyPointCoordinate {
    /// Column (`0`-indexed).
    pub x: u16,
    /// Row (`0`-indexed).
    pub y: u32,
}

/// Terminal point coordinate-system tag (`GhosttyPointTag` in `ghostty/vt/point.h`).
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GhosttyPointTag {
    /// Active screen area.
    #[default]
    Active = 0,
    /// Visible viewport.
    Viewport = 1,
    /// Full screen including scrollback.
    Screen = 2,
    /// Scrollback history only.
    History = 3,
}

/// Size report format (`GhosttySizeReportStyle` in `ghostty/vt/size_report.h`).
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GhosttySizeReportStyle {
    /// In-band size report (mode 2048).
    #[default]
    Mode2048 = 0,
    /// `CSI 14 t` pixel text area report.
    Csi14T = 1,
    /// `CSI 16 t` pixel cell size report.
    Csi16T = 2,
    /// `CSI 18 t` cell grid size report.
    Csi18T = 3,
}

/// Size report dimensions (`GhosttySizeReportSize` in `ghostty/vt/size_report.h`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GhosttySizeReportSize {
    /// Terminal height in rows.
    pub rows: u16,
    /// Terminal width in columns.
    pub columns: u16,
    /// Cell width in pixels.
    pub cell_width: u32,
    /// Cell height in pixels.
    pub cell_height: u32,
}
