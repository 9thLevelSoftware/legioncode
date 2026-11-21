//! Screen buffer cell/row storage and VT editing operations (`put_char`, scroll, erase, resize).

use serde::{Deserialize, Serialize};

use crate::abi::{GhosttyCell, GhosttyCellSemanticContent, GhosttyCellWide, GhosttyRow};
use crate::sgr::{CellAttributes, CellColor};
use crate::unicode_width::char_display_width;

/// Maximum UTF-8 bytes stored inside a single cell's grapheme cluster.
pub const MAX_CELL_GRAPHEME_BYTES: usize = 256;

/// One grid cell stored inside [`crate::parser::TerminalGrid`].
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct GridCell {
    /// UTF-8 grapheme cluster text (`""` when empty or wide spacer).
    pub text: String,
    /// Unresolved SGR styling attributes.
    pub attrs: CellAttributes,
    /// Width classification (narrow, wide, spacer tail/head).
    pub wide: GhosttyCellWide,
    /// Whether this cell belongs to an active `OSC 8` hyperlink.
    pub has_hyperlink: bool,
    /// Semantic prompt classification (`OSC 133`).
    pub semantic: GhosttyCellSemanticContent,
}

impl GridCell {
    /// Blank cell with the provided background erase color.
    pub fn blank_with_bg(bg: CellColor) -> Self {
        Self {
            text: String::new(),
            attrs: CellAttributes {
                bg,
                ..CellAttributes::default()
            },
            wide: GhosttyCellWide::Narrow,
            has_hyperlink: false,
            semantic: GhosttyCellSemanticContent::Output,
        }
    }

    /// Converts this cell to a packed 64-bit `GhosttyCell` ABI mirror.
    pub fn to_ghostty_cell(&self, style_id: u16) -> GhosttyCell {
        let mut chars = self.text.chars();
        let cp = chars.next().map_or(0, |ch| ch as u32);
        let grapheme = chars.next().is_some();
        GhosttyCell::from_codepoint(cp, style_id, self.wide, self.has_hyperlink, grapheme)
    }
}

/// One row of [`GridCell`]s inside the active screen or scrollback ring buffer.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct GridRow {
    /// Row cells (`len == cols`).
    pub cells: Vec<GridCell>,
    /// Whether content soft-wrapped from the end of this row to the next row.
    pub wrapped: bool,
    /// Whether this row is a soft-wrap continuation of the preceding row.
    pub wrap_continuation: bool,
    /// Dirty flag for incremental renderers.
    pub dirty: bool,
}

impl GridRow {
    /// Creates a row of `cols` blank cells.
    pub fn blank(cols: u16, bg: CellColor) -> Self {
        Self {
            cells: vec![GridCell::blank_with_bg(bg); usize::from(cols)],
            wrapped: false,
            wrap_continuation: false,
            dirty: true,
        }
    }

    /// Returns the row's visible text with trailing ASCII whitespace trimmed (matching Android and
    /// iOS capture normalization).
    pub fn text_trimmed(&self) -> String {
        let mut out = String::with_capacity(self.cells.len());
        for cell in &self.cells {
            if matches!(
                cell.wide,
                GhosttyCellWide::SpacerTail | GhosttyCellWide::SpacerHead
            ) {
                continue;
            }
            if cell.text.is_empty() {
                out.push(' ');
            } else {
                out.push_str(&cell.text);
            }
        }
        while out.ends_with(|ch: char| ch.is_ascii_whitespace()) {
            out.pop();
        }
        out
    }

    /// Converts row flags into a packed 64-bit `GhosttyRow` ABI mirror.
    pub const fn to_ghostty_row(&self) -> GhosttyRow {
        GhosttyRow::from_flags(self.wrapped, self.wrap_continuation, self.dirty)
    }

    /// Resizes the row to `new_cols` cells, preserving leading cells and clearing orphaned wide
    /// glyphs if truncated on a wide cell boundary.
    pub fn resize_cols(&mut self, new_cols: u16) {
        let target = usize::from(new_cols);
        self.cells.resize_with(target, GridCell::default);
        if let Some(last) = self.cells.last_mut()
            && last.wide == GhosttyCellWide::Wide
        {
            *last = GridCell::default();
        }
        self.dirty = true;
    }
}

/// Cursor snapshot saved by `DECSC` (`ESC 7`) / `CSI s` / `DECSET ?1048`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SavedCursor {
    pub cursor_x: u16,
    pub cursor_y: u16,
    pub pending_wrap: bool,
    pub attrs: CellAttributes,
    pub origin_mode: bool,
    pub wraparound: bool,
}

/// Primary or alternate terminal screen buffer state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenBuffer {
    pub rows: Vec<GridRow>,
    pub cursor_x: u16,
    pub cursor_y: u16,
    pub pending_wrap: bool,
    pub scroll_top: u16,
    pub scroll_bottom: u16,
    pub saved_cursor: Option<SavedCursor>,
}

impl ScreenBuffer {
    /// Creates a new blank screen buffer of `cols x rows`.
    pub fn new(cols: u16, rows: u16) -> Self {
        let row_count = usize::from(rows.max(1));
        Self {
            rows: (0..row_count)
                .map(|_| GridRow::blank(cols.max(1), CellColor::Default))
                .collect(),
            cursor_x: 0,
            cursor_y: 0,
            pending_wrap: false,
            scroll_top: 0,
            scroll_bottom: rows.max(1) - 1,
            saved_cursor: None,
        }
    }

    /// Clears every row in the screen buffer and resets scroll margins.
    pub fn clear_all(&mut self, cols: u16, rows: u16, bg: CellColor) {
        self.rows.clear();
        for _ in 0..usize::from(rows) {
            self.rows.push(GridRow::blank(cols, bg));
        }
        self.cursor_x = 0;
        self.cursor_y = 0;
        self.pending_wrap = false;
        self.scroll_top = 0;
        self.scroll_bottom = rows.saturating_sub(1);
    }

    /// Scrolls `[top..=bottom]` up by `count` rows, returning evicted rows when `top == 0`.
    pub fn scroll_up_region(
        &mut self,
        cols: u16,
        top: u16,
        bottom: u16,
        count: u16,
        bg: CellColor,
    ) -> Vec<GridRow> {
        let top_idx = usize::from(top);
        let bottom_idx = usize::from(bottom).min(self.rows.len().saturating_sub(1));
        if top_idx > bottom_idx || count == 0 {
            return Vec::new();
        }
        let region_len = bottom_idx - top_idx + 1;
        let shift = usize::from(count).min(region_len);
        let mut evicted = Vec::with_capacity(shift);
        for _ in 0..shift {
            let row = self.rows.remove(top_idx);
            if top_idx == 0 {
                evicted.push(row);
            }
            self.rows.insert(bottom_idx, GridRow::blank(cols, bg));
        }
        for row in &mut self.rows[top_idx..=bottom_idx] {
            row.dirty = true;
        }
        evicted
    }

    /// Scrolls `[top..=bottom]` down by `count` rows.
    pub fn scroll_down_region(
        &mut self,
        cols: u16,
        top: u16,
        bottom: u16,
        count: u16,
        bg: CellColor,
    ) {
        let top_idx = usize::from(top);
        let bottom_idx = usize::from(bottom).min(self.rows.len().saturating_sub(1));
        if top_idx > bottom_idx || count == 0 {
            return;
        }
        let region_len = bottom_idx - top_idx + 1;
        let shift = usize::from(count).min(region_len);
        for _ in 0..shift {
            self.rows.remove(bottom_idx);
            self.rows.insert(top_idx, GridRow::blank(cols, bg));
        }
        for row in &mut self.rows[top_idx..=bottom_idx] {
            row.dirty = true;
        }
    }

    /// Resizes the buffer to `new_cols x new_rows`, clamping cursor and scroll margins.
    pub fn resize(&mut self, new_cols: u16, new_rows: u16) {
        for row in &mut self.rows {
            row.resize_cols(new_cols);
        }
        let target_rows = usize::from(new_rows);
        if self.rows.len() > target_rows {
            let drop_count = self.rows.len() - target_rows;
            if self.cursor_y >= new_rows {
                self.rows.drain(0..drop_count.min(self.rows.len()));
                self.cursor_y = new_rows.saturating_sub(1);
            } else {
                self.rows.truncate(target_rows);
            }
        }
        while self.rows.len() < target_rows {
            self.rows.push(GridRow::blank(new_cols, CellColor::Default));
        }
        self.cursor_x = self.cursor_x.min(new_cols.saturating_sub(1));
        self.cursor_y = self.cursor_y.min(new_rows.saturating_sub(1));
        self.pending_wrap = false;
        self.scroll_top = 0;
        self.scroll_bottom = new_rows.saturating_sub(1);
    }
}

/// Clears any overlapping half of a wide glyph before writing at `col`.
pub fn clear_wide_overlap(row: &mut GridRow, col: usize, bg: CellColor) {
    if let Some(cell) = row.cells.get(col) {
        match cell.wide {
            GhosttyCellWide::Wide => {
                if let Some(tail) = row.cells.get_mut(col + 1)
                    && tail.wide == GhosttyCellWide::SpacerTail
                {
                    *tail = GridCell::blank_with_bg(bg);
                }
            }
            GhosttyCellWide::SpacerTail => {
                if col > 0
                    && let Some(head) = row.cells.get_mut(col - 1)
                    && head.wide == GhosttyCellWide::Wide
                {
                    *head = GridCell::blank_with_bg(bg);
                }
            }
            GhosttyCellWide::Narrow | GhosttyCellWide::SpacerHead => {}
        }
    }
}

/// Appends a zero-width combining codepoint onto the preceding glyph cell when space allows.
pub fn attach_combining_char(row: &mut GridRow, cursor_x: u16, pending_wrap: bool, ch: char) {
    if char_display_width(ch) != 0 || ch.is_control() {
        return;
    }
    let cols = row.cells.len();
    if cols == 0 {
        return;
    }
    let mut target = if pending_wrap {
        cols - 1
    } else {
        usize::from(cursor_x).saturating_sub(1)
    };
    if row.cells.get(target).map(|cell| cell.wide) == Some(GhosttyCellWide::SpacerTail)
        && target > 0
    {
        target -= 1;
    }
    if let Some(cell) = row.cells.get_mut(target)
        && !cell.text.is_empty()
        && cell.text.len() + ch.len_utf8() <= MAX_CELL_GRAPHEME_BYTES
    {
        cell.text.push(ch);
        row.dirty = true;
    }
}

/// Inserts `count` blank cells at `col`, shifting existing cells right and clearing split wide
/// characters.
pub fn insert_blank_cells(row: &mut GridRow, col: usize, count: usize, bg: CellColor) {
    let cols = row.cells.len();
    if col >= cols || count == 0 {
        return;
    }
    clear_wide_overlap(row, col, bg);
    let shift = count.min(cols - col);
    row.cells
        .splice(col..col, (0..shift).map(|_| GridCell::blank_with_bg(bg)));
    row.cells.truncate(cols);
    if let Some(last) = row.cells.last_mut()
        && last.wide == GhosttyCellWide::Wide
    {
        *last = GridCell::blank_with_bg(bg);
    }
    row.dirty = true;
}

/// Deletes `count` cells at `col`, shifting trailing cells left and padding with blanks.
pub fn delete_cells(row: &mut GridRow, col: usize, count: usize, bg: CellColor) {
    let cols = row.cells.len();
    if col >= cols || count == 0 {
        return;
    }
    clear_wide_overlap(row, col, bg);
    let remove = count.min(cols - col);
    let clear_end = (col + remove).min(cols);
    clear_wide_overlap(row, clear_end.saturating_sub(1), bg);
    row.cells.drain(col..clear_end);
    row.cells.resize_with(cols, || GridCell::blank_with_bg(bg));
    row.dirty = true;
}
