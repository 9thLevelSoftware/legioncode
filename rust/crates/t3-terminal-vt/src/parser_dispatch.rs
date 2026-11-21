//! `ESC` and `CSI` command dispatch onto [`crate::grid::TerminalGrid`].

use crate::abi::GhosttyCursorShape;
use crate::grid::TerminalGrid;
use crate::grid_edit::TerminalModes;
use crate::sgr::apply_sgr;

pub(crate) fn dispatch_esc(grid: &mut TerminalGrid, intermediates: &[u8], final_byte: u8) {
    if !intermediates.is_empty() {
        return;
    }
    match final_byte {
        b'7' => grid.save_cursor(),
        b'8' => grid.restore_cursor(),
        b'c' => grid.reset(),
        b'D' => grid.index_down(),
        b'E' => {
            grid.carriage_return();
            grid.index_down();
        }
        b'M' => grid.reverse_index(),
        _ => {}
    }
}

pub(crate) fn dispatch_csi(
    grid: &mut TerminalGrid,
    private: Option<u8>,
    intermediates: &[u8],
    params: &[(u16, bool)],
    final_byte: u8,
) {
    if intermediates == b" " && final_byte == b'q' && private.is_none() {
        apply_decscusr(grid, param_or_default(params, 0, 0));
        return;
    }
    if !intermediates.is_empty() {
        return;
    }
    if let Some(prefix) = private {
        if prefix == b'?' {
            dispatch_private_csi(grid, params, final_byte);
        }
        return;
    }

    match final_byte {
        b'A' => move_cursor_up(grid, param_nonzero(params, 0)),
        b'B' => move_cursor_down(grid, param_nonzero(params, 0)),
        b'C' => move_cursor_right(grid, param_nonzero(params, 0)),
        b'D' => move_cursor_left(grid, param_nonzero(params, 0)),
        b'E' => {
            move_cursor_down(grid, param_nonzero(params, 0));
            grid.carriage_return();
        }
        b'F' => {
            move_cursor_up(grid, param_nonzero(params, 0));
            grid.carriage_return();
        }
        b'G' | b'`' => {
            let col = param_nonzero(params, 0).saturating_sub(1);
            let max_x = grid.cols.saturating_sub(1);
            let screen = grid.active_screen_mut();
            screen.cursor_x = col.min(max_x);
            screen.pending_wrap = false;
        }
        b'H' | b'f' => {
            let row = param_nonzero(params, 0).saturating_sub(1);
            let col = param_nonzero(params, 1).saturating_sub(1);
            move_cursor_position(grid, col, row);
        }
        b'I' => grid.tab(param_nonzero(params, 0)),
        b'Z' => grid.back_tab(param_nonzero(params, 0)),
        b'd' => {
            let row = param_nonzero(params, 0).saturating_sub(1);
            let cx = grid.active_screen().cursor_x;
            move_cursor_position(grid, cx, row);
        }
        b'J' => grid.erase_in_display(param_or_default(params, 0, 0)),
        b'K' => grid.erase_in_line(param_or_default(params, 0, 0)),
        b'X' => grid.erase_chars(param_nonzero(params, 0)),
        b'L' => grid.insert_lines(param_nonzero(params, 0)),
        b'M' => grid.delete_lines(param_nonzero(params, 0)),
        b'@' => grid.insert_chars(param_nonzero(params, 0)),
        b'P' => grid.delete_chars(param_nonzero(params, 0)),
        b'S' => {
            let top = grid.active_screen().scroll_top;
            let bottom = grid.active_screen().scroll_bottom;
            grid.scroll_up_in_region(top, bottom, param_nonzero(params, 0));
        }
        b'T' => {
            let top = grid.active_screen().scroll_top;
            let bottom = grid.active_screen().scroll_bottom;
            grid.scroll_down_in_region(top, bottom, param_nonzero(params, 0));
        }
        b'r' => set_scroll_margins(grid, params),
        b's' => grid.save_cursor(),
        b'u' => grid.restore_cursor(),
        b'm' => apply_sgr(&mut grid.active_attrs, params),
        b'h' => set_ansi_modes(grid, params, true),
        b'l' => set_ansi_modes(grid, params, false),
        b'n' => handle_dsr(grid, param_or_default(params, 0, 0)),
        b'c' if param_or_default(params, 0, 0) == 0 => {
            grid.pending_responses.extend_from_slice(b"\x1b[?62;22c");
        }
        _ => {}
    }
}

fn dispatch_private_csi(grid: &mut TerminalGrid, params: &[(u16, bool)], final_byte: u8) {
    match final_byte {
        b'h' => set_dec_modes(grid, params, true),
        b'l' => set_dec_modes(grid, params, false),
        b'n' if param_or_default(params, 0, 0) == 6 => handle_dsr(grid, 6),
        _ => {}
    }
}

fn set_ansi_modes(grid: &mut TerminalGrid, params: &[(u16, bool)], enabled: bool) {
    for &(mode, _) in params {
        match mode {
            4 => grid.modes.set(TerminalModes::INSERT, enabled),
            20 => grid.modes.set(TerminalModes::LINEFEED_NEWLINE, enabled),
            _ => {}
        }
    }
}

fn set_dec_modes(grid: &mut TerminalGrid, params: &[(u16, bool)], enabled: bool) {
    for &(mode, _) in params {
        match mode {
            1 => grid.modes.set(TerminalModes::APP_CURSOR_KEYS, enabled),
            6 => {
                grid.modes.set(TerminalModes::ORIGIN, enabled);
                move_cursor_position(grid, 0, 0);
            }
            7 => grid.modes.set(TerminalModes::WRAPAROUND, enabled),
            12 => grid.modes.set(TerminalModes::CURSOR_BLINKING, enabled),
            25 => grid.modes.set(TerminalModes::CURSOR_VISIBLE, enabled),
            47 => {
                if enabled {
                    grid.enter_alternate_screen(false, false);
                } else {
                    grid.leave_alternate_screen(false);
                }
            }
            1047 => {
                if enabled {
                    grid.enter_alternate_screen(false, true);
                } else {
                    grid.leave_alternate_screen(false);
                }
            }
            1048 => {
                if enabled {
                    grid.save_cursor();
                } else {
                    grid.restore_cursor();
                }
            }
            1049 => {
                if enabled {
                    grid.enter_alternate_screen(true, true);
                } else {
                    grid.leave_alternate_screen(true);
                }
            }
            2004 => grid.modes.set(TerminalModes::BRACKETED_PASTE, enabled),
            2026 => grid.modes.set(TerminalModes::SYNC_OUTPUT, enabled),
            _ => {}
        }
    }
}

fn apply_decscusr(grid: &mut TerminalGrid, code: u16) {
    match code {
        0 | 1 => {
            grid.cursor_shape = GhosttyCursorShape::Block;
            grid.modes.set(TerminalModes::CURSOR_BLINKING, true);
        }
        2 => {
            grid.cursor_shape = GhosttyCursorShape::Block;
            grid.modes.set(TerminalModes::CURSOR_BLINKING, false);
        }
        3 => {
            grid.cursor_shape = GhosttyCursorShape::Underline;
            grid.modes.set(TerminalModes::CURSOR_BLINKING, true);
        }
        4 => {
            grid.cursor_shape = GhosttyCursorShape::Underline;
            grid.modes.set(TerminalModes::CURSOR_BLINKING, false);
        }
        5 => {
            grid.cursor_shape = GhosttyCursorShape::Bar;
            grid.modes.set(TerminalModes::CURSOR_BLINKING, true);
        }
        6 => {
            grid.cursor_shape = GhosttyCursorShape::Bar;
            grid.modes.set(TerminalModes::CURSOR_BLINKING, false);
        }
        _ => {}
    }
}

fn handle_dsr(grid: &mut TerminalGrid, code: u16) {
    match code {
        5 => grid.pending_responses.extend_from_slice(b"\x1b[0n"),
        6 => {
            let origin = grid.modes.has(TerminalModes::ORIGIN);
            let screen = grid.active_screen();
            let row_offset = if origin { screen.scroll_top } else { 0 };
            let row = screen.cursor_y.saturating_sub(row_offset) + 1;
            let col = screen.cursor_x + 1;
            let reply = format!("\x1b[{row};{col}R");
            grid.pending_responses.extend_from_slice(reply.as_bytes());
        }
        _ => {}
    }
}

fn move_cursor_up(grid: &mut TerminalGrid, count: u16) {
    let screen = grid.active_screen_mut();
    let min_y = if screen.cursor_y >= screen.scroll_top {
        screen.scroll_top
    } else {
        0
    };
    screen.cursor_y = screen.cursor_y.saturating_sub(count).max(min_y);
    screen.pending_wrap = false;
}

fn move_cursor_down(grid: &mut TerminalGrid, count: u16) {
    let max_rows = grid.rows.saturating_sub(1);
    let screen = grid.active_screen_mut();
    let max_y = if screen.cursor_y <= screen.scroll_bottom {
        screen.scroll_bottom
    } else {
        max_rows
    };
    screen.cursor_y = screen.cursor_y.saturating_add(count).min(max_y);
    screen.pending_wrap = false;
}

fn move_cursor_left(grid: &mut TerminalGrid, count: u16) {
    let screen = grid.active_screen_mut();
    screen.cursor_x = screen.cursor_x.saturating_sub(count);
    screen.pending_wrap = false;
}

fn move_cursor_right(grid: &mut TerminalGrid, count: u16) {
    let max_x = grid.cols.saturating_sub(1);
    let screen = grid.active_screen_mut();
    screen.cursor_x = screen.cursor_x.saturating_add(count).min(max_x);
    screen.pending_wrap = false;
}

fn move_cursor_position(grid: &mut TerminalGrid, col: u16, row: u16) {
    let max_x = grid.cols.saturating_sub(1);
    let max_rows = grid.rows.saturating_sub(1);
    let origin = grid.modes.has(TerminalModes::ORIGIN);
    let screen = grid.active_screen_mut();
    let target_y = if origin {
        screen
            .scroll_top
            .saturating_add(row)
            .min(screen.scroll_bottom)
    } else {
        row.min(max_rows)
    };
    screen.cursor_x = col.min(max_x);
    screen.cursor_y = target_y;
    screen.pending_wrap = false;
}

fn set_scroll_margins(grid: &mut TerminalGrid, params: &[(u16, bool)]) {
    let rows = grid.rows;
    let top = param_nonzero(params, 0).saturating_sub(1);
    let bottom_raw = param_or_default(params, 1, 0);
    let bottom = if bottom_raw == 0 {
        rows.saturating_sub(1)
    } else {
        bottom_raw.saturating_sub(1).min(rows.saturating_sub(1))
    };
    if top < bottom {
        let screen = grid.active_screen_mut();
        screen.scroll_top = top;
        screen.scroll_bottom = bottom;
        move_cursor_position(grid, 0, 0);
    }
}

fn param_or_default(params: &[(u16, bool)], index: usize, default: u16) -> u16 {
    params.get(index).map_or(default, |&(val, _)| val)
}

fn param_nonzero(params: &[(u16, bool)], index: usize) -> u16 {
    let val = param_or_default(params, index, 1);
    if val == 0 { 1 } else { val }
}
