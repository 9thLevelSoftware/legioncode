//! Cursor motion, character placement, scrolling, and alternate-screen transitions for
//! [`crate::grid::TerminalGrid`].

use crate::abi::GhosttyCellWide;
use crate::grid::TerminalGrid;
use crate::grid_edit::TerminalModes;
use crate::grid_ops::{GridCell, SavedCursor, attach_combining_char, clear_wide_overlap};
use crate::unicode_width::char_display_width;

impl TerminalGrid {
    pub(crate) fn put_char(&mut self, ch: char) {
        let width = char_display_width(ch);
        if width == 0 {
            let pending = self.active_screen().pending_wrap;
            let cx = self.active_screen().cursor_x;
            let cy = usize::from(self.active_screen().cursor_y);
            if let Some(row) = self.active_screen_mut().rows.get_mut(cy) {
                attach_combining_char(row, cx, pending, ch);
            }
            return;
        }
        let effective_width = if width == 2 && self.cols < 2 {
            1
        } else {
            width
        };
        if self.active_screen().pending_wrap {
            self.apply_pending_wrap();
        } else if effective_width == 2 && self.active_screen().cursor_x + 1 >= self.cols {
            self.wrap_before_wide_char();
        }

        let attrs = self.active_attrs;
        let hyperlink = self.active_hyperlink;
        let insert_mode = self.modes.has(TerminalModes::INSERT);
        let cols = self.cols;
        let wraparound = self.modes.has(TerminalModes::WRAPAROUND);
        let cx = usize::from(self.active_screen().cursor_x);
        let cy = usize::from(self.active_screen().cursor_y);
        if let Some(row) = self.active_screen_mut().rows.get_mut(cy) {
            if insert_mode {
                crate::grid_ops::insert_blank_cells(
                    row,
                    cx,
                    usize::from(effective_width),
                    attrs.bg,
                );
            }
            clear_wide_overlap(row, cx, attrs.bg);
            if effective_width == 2 {
                clear_wide_overlap(row, cx + 1, attrs.bg);
            }
            if let Some(cell) = row.cells.get_mut(cx) {
                cell.text.clear();
                cell.text.push(ch);
                cell.attrs = attrs;
                cell.wide = if effective_width == 2 {
                    GhosttyCellWide::Wide
                } else {
                    GhosttyCellWide::Narrow
                };
                cell.has_hyperlink = hyperlink;
            }
            if effective_width == 2
                && let Some(tail) = row.cells.get_mut(cx + 1)
            {
                *tail = GridCell::blank_with_bg(attrs.bg);
                tail.wide = GhosttyCellWide::SpacerTail;
                tail.has_hyperlink = hyperlink;
            }
            row.dirty = true;
        }

        let next_x = self.active_screen().cursor_x + u16::from(effective_width);
        let screen = self.active_screen_mut();
        if next_x >= cols {
            screen.cursor_x = cols.saturating_sub(1);
            screen.pending_wrap = wraparound;
        } else {
            screen.cursor_x = next_x;
            screen.pending_wrap = false;
        }
    }

    fn wrap_before_wide_char(&mut self) {
        if !self.modes.has(TerminalModes::WRAPAROUND) {
            self.active_screen_mut().cursor_x = self.cols.saturating_sub(2);
            return;
        }
        let cy = usize::from(self.active_screen().cursor_y);
        let bg = self.active_attrs.bg;
        if let Some(row) = self.active_screen_mut().rows.get_mut(cy) {
            let last = row.cells.len().saturating_sub(1);
            clear_wide_overlap(row, last, bg);
            if let Some(cell) = row.cells.get_mut(last) {
                *cell = GridCell::blank_with_bg(bg);
                cell.wide = GhosttyCellWide::SpacerHead;
            }
            row.wrapped = true;
            row.dirty = true;
        }
        self.line_feed();
        let cy_next = usize::from(self.active_screen().cursor_y);
        if let Some(row) = self.active_screen_mut().rows.get_mut(cy_next) {
            row.wrap_continuation = true;
        }
        self.active_screen_mut().cursor_x = 0;
    }

    fn apply_pending_wrap(&mut self) {
        if !self.modes.has(TerminalModes::WRAPAROUND) {
            self.active_screen_mut().pending_wrap = false;
            return;
        }
        let cy = usize::from(self.active_screen().cursor_y);
        if let Some(row) = self.active_screen_mut().rows.get_mut(cy) {
            row.wrapped = true;
            row.dirty = true;
        }
        self.line_feed();
        let new_cy = usize::from(self.active_screen().cursor_y);
        if let Some(row) = self.active_screen_mut().rows.get_mut(new_cy) {
            row.wrap_continuation = true;
        }
        let screen = self.active_screen_mut();
        screen.cursor_x = 0;
        screen.pending_wrap = false;
    }

    pub(crate) fn carriage_return(&mut self) {
        let screen = self.active_screen_mut();
        screen.cursor_x = 0;
        screen.pending_wrap = false;
    }

    pub(crate) fn backspace(&mut self) {
        let screen = self.active_screen_mut();
        screen.cursor_x = screen.cursor_x.saturating_sub(1);
        screen.pending_wrap = false;
    }

    pub(crate) fn tab(&mut self, count: u16) {
        let max_col = self.cols.saturating_sub(1);
        let screen = self.active_screen_mut();
        let mut x = screen.cursor_x;
        for _ in 0..count.max(1) {
            let next = ((x / 8) + 1) * 8;
            x = next.min(max_col);
        }
        screen.cursor_x = x;
        screen.pending_wrap = false;
    }

    pub(crate) fn back_tab(&mut self, count: u16) {
        let screen = self.active_screen_mut();
        let mut x = screen.cursor_x;
        for _ in 0..count.max(1) {
            if x == 0 {
                break;
            }
            x = ((x - 1) / 8) * 8;
        }
        screen.cursor_x = x;
        screen.pending_wrap = false;
    }

    pub(crate) fn line_feed(&mut self) {
        self.index_down();
        if self.modes.has(TerminalModes::LINEFEED_NEWLINE) {
            self.active_screen_mut().cursor_x = 0;
        }
    }

    pub(crate) fn index_down(&mut self) {
        let top = self.active_screen().scroll_top;
        let bottom = self.active_screen().scroll_bottom;
        let cy = self.active_screen().cursor_y;
        self.active_screen_mut().pending_wrap = false;
        if cy == bottom {
            self.scroll_up_in_region(top, bottom, 1);
        } else if cy + 1 < self.rows {
            self.active_screen_mut().cursor_y = cy + 1;
        }
    }

    pub(crate) fn reverse_index(&mut self) {
        let top = self.active_screen().scroll_top;
        let bottom = self.active_screen().scroll_bottom;
        let cy = self.active_screen().cursor_y;
        self.active_screen_mut().pending_wrap = false;
        if cy == top {
            let bg = self.active_attrs.bg;
            let cols = self.cols;
            self.active_screen_mut()
                .scroll_down_region(cols, top, bottom, 1, bg);
        } else {
            self.active_screen_mut().cursor_y = cy.saturating_sub(1);
        }
    }

    pub(crate) fn scroll_up_in_region(&mut self, top: u16, bottom: u16, count: u16) {
        let bg = self.active_attrs.bg;
        let cols = self.cols;
        let is_alt = self.use_alternate;
        let evicted = self
            .active_screen_mut()
            .scroll_up_region(cols, top, bottom, count, bg);
        if !is_alt {
            for row in evicted {
                self.push_scrollback(row);
            }
        }
    }

    pub(crate) fn scroll_down_in_region(&mut self, top: u16, bottom: u16, count: u16) {
        let bg = self.active_attrs.bg;
        let cols = self.cols;
        self.active_screen_mut()
            .scroll_down_region(cols, top, bottom, count, bg);
    }

    pub(crate) fn save_cursor(&mut self) {
        let screen = self.active_screen();
        let saved = SavedCursor {
            cursor_x: screen.cursor_x,
            cursor_y: screen.cursor_y,
            pending_wrap: screen.pending_wrap,
            attrs: self.active_attrs,
            origin_mode: self.modes.has(TerminalModes::ORIGIN),
            wraparound: self.modes.has(TerminalModes::WRAPAROUND),
        };
        self.active_screen_mut().saved_cursor = Some(saved);
    }

    pub(crate) fn restore_cursor(&mut self) {
        let Some(saved) = self.active_screen().saved_cursor else {
            return;
        };
        let max_x = self.cols.saturating_sub(1);
        let max_y = self.rows.saturating_sub(1);
        let screen = self.active_screen_mut();
        screen.cursor_x = saved.cursor_x.min(max_x);
        screen.cursor_y = saved.cursor_y.min(max_y);
        screen.pending_wrap = saved.pending_wrap;
        self.active_attrs = saved.attrs;
        self.modes.set(TerminalModes::ORIGIN, saved.origin_mode);
        self.modes.set(TerminalModes::WRAPAROUND, saved.wraparound);
    }

    pub(crate) fn enter_alternate_screen(&mut self, save_cursor: bool, clear: bool) {
        if save_cursor {
            self.save_cursor();
        }
        if !self.use_alternate {
            self.use_alternate = true;
            self.viewport_offset_from_bottom = 0;
        }
        if clear {
            self.alternate
                .clear_all(self.cols, self.rows, self.active_attrs.bg);
        }
    }

    pub(crate) fn leave_alternate_screen(&mut self, restore_cursor: bool) {
        if self.use_alternate {
            self.use_alternate = false;
            self.viewport_offset_from_bottom = 0;
        }
        if restore_cursor {
            self.restore_cursor();
        }
    }
}
