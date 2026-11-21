//! Display/line erase, character insertion/deletion, line insertion/deletion, and packed mode
//! flags for [`crate::grid::TerminalGrid`].

use crate::grid::TerminalGrid;
use crate::grid_ops::{GridCell, GridRow, clear_wide_overlap, delete_cells, insert_blank_cells};

/// Packed terminal mode bitset so `TerminalGrid` maintains clean single-word mode state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TerminalModes(u16);

impl TerminalModes {
    pub const CURSOR_VISIBLE: u16 = 1 << 0;
    pub const CURSOR_BLINKING: u16 = 1 << 1;
    pub const WRAPAROUND: u16 = 1 << 2;
    pub const ORIGIN: u16 = 1 << 3;
    pub const APP_CURSOR_KEYS: u16 = 1 << 4;
    pub const BRACKETED_PASTE: u16 = 1 << 5;
    pub const SYNC_OUTPUT: u16 = 1 << 6;
    pub const INSERT: u16 = 1 << 7;
    pub const LINEFEED_NEWLINE: u16 = 1 << 8;

    pub const fn default_modes() -> Self {
        Self(Self::CURSOR_VISIBLE | Self::CURSOR_BLINKING | Self::WRAPAROUND)
    }

    pub const fn has(self, bit: u16) -> bool {
        (self.0 & bit) != 0
    }

    pub fn set(&mut self, bit: u16, enabled: bool) {
        if enabled {
            self.0 |= bit;
        } else {
            self.0 &= !bit;
        }
    }
}

impl Default for TerminalModes {
    fn default() -> Self {
        Self::default_modes()
    }
}

impl TerminalGrid {
    pub(crate) fn erase_in_display(&mut self, mode: u16) {
        let bg = self.active_attrs.bg;
        let cx = usize::from(self.active_screen().cursor_x);
        let cy = usize::from(self.active_screen().cursor_y);
        let cols = self.cols;
        match mode {
            0 => {
                if let Some(row) = self.active_screen_mut().rows.get_mut(cy) {
                    clear_wide_overlap(row, cx, bg);
                    for cell in row.cells.iter_mut().skip(cx) {
                        *cell = GridCell::blank_with_bg(bg);
                    }
                    row.wrapped = false;
                    row.dirty = true;
                }
                for row in self.active_screen_mut().rows.iter_mut().skip(cy + 1) {
                    *row = GridRow::blank(cols, bg);
                }
            }
            1 => {
                for row in self.active_screen_mut().rows.iter_mut().take(cy) {
                    *row = GridRow::blank(cols, bg);
                }
                if let Some(row) = self.active_screen_mut().rows.get_mut(cy) {
                    clear_wide_overlap(row, cx, bg);
                    for cell in row.cells.iter_mut().take(cx + 1) {
                        *cell = GridCell::blank_with_bg(bg);
                    }
                    row.dirty = true;
                }
            }
            2 | 3 => {
                for row in &mut self.active_screen_mut().rows {
                    *row = GridRow::blank(cols, bg);
                }
                if mode == 3 && !self.use_alternate {
                    self.clear_scrollback();
                }
            }
            _ => {}
        }
        self.active_screen_mut().pending_wrap = false;
    }

    pub(crate) fn erase_in_line(&mut self, mode: u16) {
        let bg = self.active_attrs.bg;
        let cx = usize::from(self.active_screen().cursor_x);
        let cy = usize::from(self.active_screen().cursor_y);
        let cols = self.cols;
        if let Some(row) = self.active_screen_mut().rows.get_mut(cy) {
            match mode {
                0 => {
                    clear_wide_overlap(row, cx, bg);
                    for cell in row.cells.iter_mut().skip(cx) {
                        *cell = GridCell::blank_with_bg(bg);
                    }
                    row.wrapped = false;
                }
                1 => {
                    clear_wide_overlap(row, cx, bg);
                    for cell in row.cells.iter_mut().take(cx + 1) {
                        *cell = GridCell::blank_with_bg(bg);
                    }
                }
                2 => *row = GridRow::blank(cols, bg),
                _ => return,
            }
            row.dirty = true;
        }
        self.active_screen_mut().pending_wrap = false;
    }

    pub(crate) fn erase_chars(&mut self, count: u16) {
        let bg = self.active_attrs.bg;
        let cx = usize::from(self.active_screen().cursor_x);
        let cy = usize::from(self.active_screen().cursor_y);
        let n = usize::from(count.max(1));
        if let Some(row) = self.active_screen_mut().rows.get_mut(cy) {
            let end = (cx + n).min(row.cells.len());
            clear_wide_overlap(row, cx, bg);
            if end > cx {
                clear_wide_overlap(row, end - 1, bg);
                for cell in &mut row.cells[cx..end] {
                    *cell = GridCell::blank_with_bg(bg);
                }
                row.dirty = true;
            }
        }
        self.active_screen_mut().pending_wrap = false;
    }

    pub(crate) fn insert_chars(&mut self, count: u16) {
        let bg = self.active_attrs.bg;
        let cx = usize::from(self.active_screen().cursor_x);
        let cy = usize::from(self.active_screen().cursor_y);
        if let Some(row) = self.active_screen_mut().rows.get_mut(cy) {
            insert_blank_cells(row, cx, usize::from(count.max(1)), bg);
        }
        self.active_screen_mut().pending_wrap = false;
    }

    pub(crate) fn delete_chars(&mut self, count: u16) {
        let bg = self.active_attrs.bg;
        let cx = usize::from(self.active_screen().cursor_x);
        let cy = usize::from(self.active_screen().cursor_y);
        if let Some(row) = self.active_screen_mut().rows.get_mut(cy) {
            delete_cells(row, cx, usize::from(count.max(1)), bg);
        }
        self.active_screen_mut().pending_wrap = false;
    }

    pub(crate) fn insert_lines(&mut self, count: u16) {
        let top = self.active_screen().scroll_top;
        let bottom = self.active_screen().scroll_bottom;
        let cy = self.active_screen().cursor_y;
        if cy < top || cy > bottom {
            return;
        }
        let bg = self.active_attrs.bg;
        let cols = self.cols;
        self.active_screen_mut()
            .scroll_down_region(cols, cy, bottom, count.max(1), bg);
        self.active_screen_mut().cursor_x = 0;
        self.active_screen_mut().pending_wrap = false;
    }

    pub(crate) fn delete_lines(&mut self, count: u16) {
        let top = self.active_screen().scroll_top;
        let bottom = self.active_screen().scroll_bottom;
        let cy = self.active_screen().cursor_y;
        if cy < top || cy > bottom {
            return;
        }
        let bg = self.active_attrs.bg;
        let cols = self.cols;
        self.active_screen_mut()
            .scroll_up_region(cols, cy, bottom, count.max(1), bg);
        self.active_screen_mut().cursor_x = 0;
        self.active_screen_mut().pending_wrap = false;
    }
}
