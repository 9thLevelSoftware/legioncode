//! Bounded terminal grid (`TerminalGrid`) maintaining primary/alternate screens, scrollback ring
//! buffer, cursor geometry, active attributes, `OSC` metadata, and theme colors.

use std::collections::VecDeque;

use crate::abi::{GhosttyCursorShape, GhosttyMode};
use crate::error::VtError;
use crate::grid_edit::TerminalModes;
use crate::grid_ops::{GridRow, ScreenBuffer};
use crate::palette::{AnsiPalette256, Rgba, TerminalThemeConfig};
use crate::parser::VtParser;
use crate::sgr::{CellAttributes, CellColor};

/// Maximum terminal columns allowed when constructing or resizing a grid.
pub const MAX_GRID_COLS: u16 = 1024;
/// Maximum terminal rows allowed when constructing or resizing a grid.
pub const MAX_GRID_ROWS: u16 = 512;
/// Default scrollback history capacity in rows.
pub const DEFAULT_MAX_SCROLLBACK: usize = 2000;

/// Bounded terminal state machine backing streaming VT parsing, scrollback, and snapshots.
#[derive(Debug, Clone)]
pub struct TerminalGrid {
    pub(crate) cols: u16,
    pub(crate) rows: u16,
    pub(crate) primary: ScreenBuffer,
    pub(crate) alternate: ScreenBuffer,
    pub(crate) use_alternate: bool,
    scrollback: VecDeque<GridRow>,
    max_scrollback: usize,
    pub(crate) viewport_offset_from_bottom: usize,
    pub(crate) active_attrs: CellAttributes,
    pub(crate) active_hyperlink: bool,
    pub(crate) modes: TerminalModes,
    pub(crate) cursor_shape: GhosttyCursorShape,
    pub(crate) title: String,
    pub(crate) icon_title: String,
    pub(crate) pwd: String,
    pub(crate) foreground: Option<Rgba>,
    pub(crate) background: Option<Rgba>,
    pub(crate) cursor_color: Option<Rgba>,
    default_foreground: Rgba,
    default_background: Rgba,
    default_cursor_color: Rgba,
    palette: AnsiPalette256,
    pub(crate) pending_responses: Vec<u8>,
    pub(crate) bell_count: u64,
    parser: VtParser,
}

impl TerminalGrid {
    /// Creates a terminal grid clamped to `1..=1024` columns and `1..=512` rows with default
    /// `2000`-line scrollback.
    pub fn new(cols: u16, rows: u16) -> Self {
        Self::with_scrollback(cols, rows, DEFAULT_MAX_SCROLLBACK)
    }

    /// Validates dimensions strictly before constructing a [`TerminalGrid`].
    pub fn try_new(cols: u16, rows: u16) -> Result<Self, VtError> {
        if cols == 0 || rows == 0 || cols > MAX_GRID_COLS || rows > MAX_GRID_ROWS {
            return Err(VtError::InvalidDimensions { cols, rows });
        }
        Ok(Self::new(cols, rows))
    }

    /// Creates a terminal grid with an explicit scrollback line cap.
    pub fn with_scrollback(cols: u16, rows: u16, max_scrollback: usize) -> Self {
        let cols = cols.clamp(1, MAX_GRID_COLS);
        let rows = rows.clamp(1, MAX_GRID_ROWS);
        Self {
            cols,
            rows,
            primary: ScreenBuffer::new(cols, rows),
            alternate: ScreenBuffer::new(cols, rows),
            use_alternate: false,
            scrollback: VecDeque::new(),
            max_scrollback,
            viewport_offset_from_bottom: 0,
            active_attrs: CellAttributes::default(),
            active_hyperlink: false,
            modes: TerminalModes::default_modes(),
            cursor_shape: GhosttyCursorShape::Block,
            title: String::new(),
            icon_title: String::new(),
            pwd: String::new(),
            foreground: None,
            background: None,
            cursor_color: None,
            default_foreground: Rgba::rgb(0xD1, 0xD5, 0xDA),
            default_background: Rgba::rgb(0x24, 0x29, 0x2E),
            default_cursor_color: Rgba::rgb(0x00, 0x9F, 0xFF),
            palette: AnsiPalette256::new(),
            pending_responses: Vec::new(),
            bell_count: 0,
            parser: VtParser::new(),
        }
    }

    /// Feeds raw VT/UTF-8 bytes incrementally through the streaming parser.
    pub fn feed(&mut self, bytes: &[u8]) {
        let mut parser = std::mem::take(&mut self.parser);
        parser.feed(bytes, self);
        self.parser = parser;
    }

    /// Convenience wrapper feeding a UTF-8 string slice.
    pub fn feed_str(&mut self, text: &str) {
        self.feed(text.as_bytes());
    }

    /// Performs a full terminal reset (`RIS` / `ESC c`) preserving viewport dimensions and theme.
    pub fn reset(&mut self) {
        self.primary
            .clear_all(self.cols, self.rows, CellColor::Default);
        self.alternate
            .clear_all(self.cols, self.rows, CellColor::Default);
        self.use_alternate = false;
        self.scrollback.clear();
        self.viewport_offset_from_bottom = 0;
        self.active_attrs.reset();
        self.active_hyperlink = false;
        self.modes = TerminalModes::default_modes();
        self.cursor_shape = GhosttyCursorShape::Block;
        self.title.clear();
        self.icon_title.clear();
        self.pwd.clear();
        self.foreground = None;
        self.background = None;
        self.cursor_color = None;
        self.palette.clear();
        self.pending_responses.clear();
        self.parser.reset();
    }

    /// Resizes both primary and alternate screens, clamping to `1..=1024 x 1..=512`.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        let next_cols = cols.clamp(1, MAX_GRID_COLS);
        let next_rows = rows.clamp(1, MAX_GRID_ROWS);
        if next_cols == self.cols && next_rows == self.rows {
            return;
        }
        self.cols = next_cols;
        self.rows = next_rows;
        self.primary.resize(next_cols, next_rows);
        self.alternate.resize(next_cols, next_rows);
        self.viewport_offset_from_bottom =
            self.viewport_offset_from_bottom.min(self.scrollback.len());
    }

    /// Applies default theme colors and sparse palette overrides (`BUG-25`).
    pub fn apply_theme(&mut self, theme: &TerminalThemeConfig) {
        if let Some(fg) = theme.foreground {
            self.default_foreground = fg;
        }
        if let Some(bg) = theme.background {
            self.default_background = bg;
        }
        if let Some(cursor) = theme.cursor_color {
            self.default_cursor_color = cursor;
        }
        let pairs = theme.palette.override_pairs();
        self.palette.apply_theme_overrides(&pairs);
    }

    /// Active column count.
    pub const fn cols(&self) -> u16 {
        self.cols
    }

    /// Active row count.
    pub const fn rows(&self) -> u16 {
        self.rows
    }

    /// Cursor column (`0`-indexed) on the active screen.
    pub fn cursor_x(&self) -> u16 {
        self.active_screen().cursor_x
    }

    /// Cursor row (`0`-indexed) on the active screen.
    pub fn cursor_y(&self) -> u16 {
        self.active_screen().cursor_y
    }

    /// Whether cursor visibility (`DECTCEM` mode 25) is enabled.
    pub const fn cursor_visible(&self) -> bool {
        self.modes.has(TerminalModes::CURSOR_VISIBLE)
    }

    /// Whether cursor blinking (mode 12 / `DECSCUSR`) is enabled.
    pub const fn cursor_blinking(&self) -> bool {
        self.modes.has(TerminalModes::CURSOR_BLINKING)
    }

    /// Current visual cursor shape.
    pub const fn cursor_shape(&self) -> GhosttyCursorShape {
        self.cursor_shape
    }

    /// Current window title (`OSC 0` / `OSC 2`).
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Current icon title (`OSC 0` / `OSC 1`).
    pub fn icon_title(&self) -> &str {
        &self.icon_title
    }

    /// Current working directory (`OSC 7`).
    pub fn pwd(&self) -> &str {
        &self.pwd
    }

    /// Whether the alternate screen buffer (`?47`/`?1047`/`?1049`) is active.
    pub const fn is_alternate_screen(&self) -> bool {
        self.use_alternate
    }

    /// Whether bracketed paste (`?2004`) is active.
    pub const fn bracketed_paste_enabled(&self) -> bool {
        self.modes.has(TerminalModes::BRACKETED_PASTE)
    }

    /// Queries the boolean state of a packed [`GhosttyMode`].
    pub const fn mode(&self, mode: GhosttyMode) -> bool {
        match (mode.is_ansi(), mode.value()) {
            (true, 4) => self.modes.has(TerminalModes::INSERT),
            (true, 20) => self.modes.has(TerminalModes::LINEFEED_NEWLINE),
            (false, 1) => self.modes.has(TerminalModes::APP_CURSOR_KEYS),
            (false, 6) => self.modes.has(TerminalModes::ORIGIN),
            (false, 7) => self.modes.has(TerminalModes::WRAPAROUND),
            (false, 12) => self.modes.has(TerminalModes::CURSOR_BLINKING),
            (false, 25) => self.modes.has(TerminalModes::CURSOR_VISIBLE),
            (false, 47 | 1047 | 1049) => self.use_alternate,
            (false, 2004) => self.modes.has(TerminalModes::BRACKETED_PASTE),
            (false, 2026) => self.modes.has(TerminalModes::SYNC_OUTPUT),
            _ => false,
        }
    }

    /// Number of retained scrollback rows on the primary screen.
    pub fn scrollback_len(&self) -> usize {
        self.scrollback.len()
    }

    /// Returns a scrollback row (`0` = oldest retained line).
    pub fn scrollback_row(&self, index: usize) -> Option<&GridRow> {
        self.scrollback.get(index)
    }

    /// Current number of lines scrolled up from the live bottom (`0` = pinned to bottom).
    pub const fn scroll_offset_from_bottom(&self) -> usize {
        self.viewport_offset_from_bottom
    }

    /// Borrows the 256-color ANSI palette.
    pub const fn palette(&self) -> &AnsiPalette256 {
        &self.palette
    }

    /// Mutably borrows the 256-color ANSI palette.
    pub const fn palette_mut(&mut self) -> &mut AnsiPalette256 {
        &mut self.palette
    }

    /// Effective foreground color (`OSC 10` override or theme default).
    pub fn effective_foreground(&self) -> Rgba {
        self.foreground.unwrap_or(self.default_foreground)
    }

    /// Effective background color (`OSC 11` override or theme default).
    pub fn effective_background(&self) -> Rgba {
        self.background.unwrap_or(self.default_background)
    }

    /// Effective cursor color (`OSC 12` override or theme default).
    pub fn effective_cursor_color(&self) -> Rgba {
        self.cursor_color.unwrap_or(self.default_cursor_color)
    }

    /// Drains queued PTY reply bytes (such as `DSR` `5n`/`6n`, `DA1`, or `OSC` queries).
    pub fn drain_responses(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.pending_responses)
    }

    /// Total `BEL` (`0x07`) events received.
    pub const fn bell_count(&self) -> u64 {
        self.bell_count
    }

    pub(crate) fn active_screen(&self) -> &ScreenBuffer {
        if self.use_alternate {
            &self.alternate
        } else {
            &self.primary
        }
    }

    pub(crate) fn active_screen_mut(&mut self) -> &mut ScreenBuffer {
        if self.use_alternate {
            &mut self.alternate
        } else {
            &mut self.primary
        }
    }

    pub(crate) fn push_scrollback(&mut self, row: GridRow) {
        if self.max_scrollback == 0 {
            return;
        }
        if self.scrollback.len() >= self.max_scrollback {
            self.scrollback.pop_front();
        }
        self.scrollback.push_back(row);
        if self.viewport_offset_from_bottom > 0 {
            self.viewport_offset_from_bottom =
                (self.viewport_offset_from_bottom + 1).min(self.scrollback.len());
        }
    }

    pub(crate) fn clear_scrollback(&mut self) {
        self.scrollback.clear();
        self.viewport_offset_from_bottom = 0;
    }

    pub(crate) fn viewport_row(&self, viewport_row: usize) -> Option<&GridRow> {
        if self.use_alternate || self.viewport_offset_from_bottom == 0 {
            return self.active_screen().rows.get(viewport_row);
        }
        let sb_len = self.scrollback.len();
        let start = sb_len.saturating_sub(self.viewport_offset_from_bottom);
        let abs_idx = start + viewport_row;
        if abs_idx < sb_len {
            self.scrollback.get(abs_idx)
        } else {
            self.primary.rows.get(abs_idx - sb_len)
        }
    }
}
