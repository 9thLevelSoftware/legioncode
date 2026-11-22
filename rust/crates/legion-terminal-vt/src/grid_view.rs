//! Viewport scrolling, scrollbar reporting, plain-text capture, and [`TerminalSnapshot`] export
//! for [`crate::grid::TerminalGrid`].

use crate::abi::{
    GhosttyStyleFlags, GhosttyTerminalScrollViewport, GhosttyTerminalScrollViewportTag,
    GhosttyTerminalScrollbar, GhosttyViewport,
};
use crate::grid::TerminalGrid;
use crate::grid_ops::{GridCell, GridRow};
use crate::palette::Rgba;
use crate::sgr::CellColor;
use crate::snapshot::{SnapshotCell, TerminalSnapshot};

impl TerminalGrid {
    /// Borrows the cell at active-screen `(col, row)`.
    pub fn cell(&self, col: u16, row: u16) -> Option<&GridCell> {
        self.active_screen()
            .rows
            .get(usize::from(row))
            .and_then(|r| r.cells.get(usize::from(col)))
    }

    /// Returns trimmed text for active-screen `row`.
    pub fn row_text(&self, row: u16) -> Option<String> {
        self.active_screen()
            .rows
            .get(usize::from(row))
            .map(GridRow::text_trimmed)
    }

    /// Joins visible viewport rows with `\n` and trims trailing ASCII whitespace per row.
    pub fn viewport_text(&self) -> String {
        let mut lines = Vec::with_capacity(usize::from(self.rows));
        for row_idx in 0..usize::from(self.rows) {
            if let Some(row) = self.viewport_row(row_idx) {
                lines.push(row.text_trimmed());
            }
        }
        lines.join("\n")
    }

    /// Scrolls the viewport using Ghostty's [`GhosttyTerminalScrollViewport`] command.
    pub fn scroll_viewport(&mut self, scroll: GhosttyTerminalScrollViewport) {
        if self.use_alternate {
            self.viewport_offset_from_bottom = 0;
            return;
        }
        let max_up = self.scrollback_len();
        match scroll.tag {
            GhosttyTerminalScrollViewportTag::Top => {
                self.viewport_offset_from_bottom = max_up;
            }
            GhosttyTerminalScrollViewportTag::Bottom => {
                self.viewport_offset_from_bottom = 0;
            }
            GhosttyTerminalScrollViewportTag::Delta => {
                let delta = scroll.value.as_delta();
                if delta < 0 {
                    let up = delta.unsigned_abs();
                    self.viewport_offset_from_bottom = self
                        .viewport_offset_from_bottom
                        .saturating_add(up)
                        .min(max_up);
                } else {
                    let down = delta.unsigned_abs();
                    self.viewport_offset_from_bottom =
                        self.viewport_offset_from_bottom.saturating_sub(down);
                }
            }
            GhosttyTerminalScrollViewportTag::Row => {
                let row = scroll.value.as_row().min(max_up);
                self.viewport_offset_from_bottom = max_up.saturating_sub(row);
            }
        }
    }

    /// Returns Ghostty scrollbar metrics (`total`, `offset`, `len`).
    pub fn scrollbar(&self) -> GhosttyTerminalScrollbar {
        let len = u64::from(self.rows);
        if self.use_alternate {
            return GhosttyTerminalScrollbar {
                total: len,
                offset: 0,
                len,
            };
        }
        let sb = self.scrollback_len();
        let offset = sb.saturating_sub(self.viewport_offset_from_bottom);
        GhosttyTerminalScrollbar {
            total: u64::try_from(sb).unwrap_or(u64::MAX).saturating_add(len),
            offset: u64::try_from(offset).unwrap_or(u64::MAX),
            len,
        }
    }

    /// Returns a C-ABI [`GhosttyViewport`] summary of the current viewport.
    pub fn viewport(&self) -> GhosttyViewport {
        let sb = self.scrollbar();
        let in_viewport = self.use_alternate || self.viewport_offset_from_bottom == 0;
        GhosttyViewport {
            cols: self.cols,
            rows: self.rows,
            scroll_offset: sb.offset,
            total_rows: sb.total,
            cursor_x: if in_viewport {
                self.cursor_x()
            } else {
                u16::MAX
            },
            cursor_y: if in_viewport {
                self.cursor_y()
            } else {
                u16::MAX
            },
            cursor_shape: self.cursor_shape,
            cursor_visible: self.cursor_visible() && in_viewport,
            cursor_blinking: self.cursor_blinking(),
            alternate_screen: self.use_alternate,
        }
    }

    /// Builds a resolved [`TerminalSnapshot`] ready for rendering or `"LCVT"` serialization.
    pub fn snapshot(&self) -> TerminalSnapshot {
        let fg_default = self.effective_foreground();
        let bg_default = self.effective_background();
        let cursor_color = self.effective_cursor_color();
        let in_viewport = self.use_alternate || self.viewport_offset_from_bottom == 0;
        let mut cells = Vec::with_capacity(usize::from(self.cols) * usize::from(self.rows));

        for row_idx in 0..usize::from(self.rows) {
            if let Some(row) = self.viewport_row(row_idx) {
                for cell in &row.cells {
                    let mut fg = self.resolve_cell_color(cell.attrs.fg, fg_default);
                    let mut bg = self.resolve_cell_color(cell.attrs.bg, bg_default);
                    if cell.attrs.flags.contains(GhosttyStyleFlags::INVERSE) {
                        std::mem::swap(&mut fg, &mut bg);
                    }
                    if cell.attrs.flags.contains(GhosttyStyleFlags::FAINT) {
                        fg = fg.blend_over(bg, 155);
                    }
                    cells.push(SnapshotCell {
                        foreground: fg,
                        background: bg,
                        flags: cell.attrs.flags,
                        text: cell.text.clone(),
                    });
                }
            }
        }

        TerminalSnapshot {
            cols: self.cols,
            rows: self.rows,
            cursor_x: if in_viewport {
                self.cursor_x()
            } else {
                u16::MAX
            },
            cursor_y: if in_viewport {
                self.cursor_y()
            } else {
                u16::MAX
            },
            cursor_visible: self.cursor_visible() && in_viewport,
            cursor_style: self.cursor_shape,
            cursor_blinking: self.cursor_blinking(),
            alternate_screen: self.use_alternate,
            foreground: fg_default,
            background: bg_default,
            cursor_color,
            palette: self.palette().clone(),
            cells,
        }
    }

    fn resolve_cell_color(&self, color: CellColor, default_color: Rgba) -> Rgba {
        match color {
            CellColor::Default => default_color,
            CellColor::Palette(idx) => self.palette().resolve(idx),
            CellColor::Rgb(rgb) => Rgba::from_rgb(rgb),
        }
    }
}
