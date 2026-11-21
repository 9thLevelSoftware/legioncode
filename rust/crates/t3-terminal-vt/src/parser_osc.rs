//! `OSC` (Operating System Command) dispatch onto [`crate::grid::TerminalGrid`].

use crate::grid::TerminalGrid;
use crate::palette::parse_color;

pub(crate) fn dispatch_osc(grid: &mut TerminalGrid, raw: &[u8]) {
    let text = String::from_utf8_lossy(raw);
    let Some((cmd_raw, payload)) = text.split_once(';') else {
        match text.trim() {
            "104" => grid.palette_mut().clear(),
            "110" => grid.foreground = None,
            "111" => grid.background = None,
            "112" => grid.cursor_color = None,
            _ => {}
        }
        return;
    };
    let Ok(cmd) = cmd_raw.trim().parse::<u16>() else {
        return;
    };
    match cmd {
        0 => {
            let clean = sanitize_osc_text(payload);
            grid.icon_title.clone_from(&clean);
            grid.title = clean;
        }
        1 => grid.icon_title = sanitize_osc_text(payload),
        2 => grid.title = sanitize_osc_text(payload),
        4 => dispatch_osc_palette(grid, payload),
        7 => grid.pwd = sanitize_osc_text(payload),
        8 => {
            if let Some((_, uri)) = payload.split_once(';') {
                grid.active_hyperlink = !uri.trim().is_empty();
            } else {
                grid.active_hyperlink = false;
            }
        }
        10 => dispatch_osc_dynamic_color(grid, 10, payload),
        11 => dispatch_osc_dynamic_color(grid, 11, payload),
        12 => dispatch_osc_dynamic_color(grid, 12, payload),
        104 => {
            if payload.trim().is_empty() {
                grid.palette_mut().clear();
            } else {
                for part in payload.split(';') {
                    if let Ok(idx) = part.trim().parse::<u8>() {
                        grid.palette_mut().set_override(idx, None);
                    }
                }
            }
        }
        110 => grid.foreground = None,
        111 => grid.background = None,
        112 => grid.cursor_color = None,
        _ => {}
    }
}

fn dispatch_osc_palette(grid: &mut TerminalGrid, payload: &str) {
    let mut parts = payload.split(';');
    while let (Some(idx_raw), Some(spec)) = (parts.next(), parts.next()) {
        let Ok(idx) = idx_raw.trim().parse::<u8>() else {
            continue;
        };
        if spec.trim() == "?" {
            let color = grid.palette().resolve(idx);
            let reply = format_osc_color_reply(&format!("4;{idx}"), color.r, color.g, color.b);
            grid.pending_responses.extend_from_slice(reply.as_bytes());
        } else if let Ok(color) = parse_color(spec) {
            grid.palette_mut().set_override(idx, Some(color));
        }
    }
}

fn dispatch_osc_dynamic_color(grid: &mut TerminalGrid, cmd: u16, payload: &str) {
    let spec = payload.trim();
    if spec == "?" {
        let color = match cmd {
            10 => grid.effective_foreground(),
            11 => grid.effective_background(),
            _ => grid.effective_cursor_color(),
        };
        let reply = format_osc_color_reply(&cmd.to_string(), color.r, color.g, color.b);
        grid.pending_responses.extend_from_slice(reply.as_bytes());
    } else if let Ok(color) = parse_color(spec) {
        match cmd {
            10 => grid.foreground = Some(color),
            11 => grid.background = Some(color),
            12 => grid.cursor_color = Some(color),
            _ => {}
        }
    }
}

fn format_osc_color_reply(prefix: &str, r: u8, g: u8, b: u8) -> String {
    let rr = u16::from(r) * 0x0101;
    let gg = u16::from(g) * 0x0101;
    let bb = u16::from(b) * 0x0101;
    format!("\x1b]{prefix};rgb:{rr:04x}/{gg:04x}/{bb:04x}\x1b\\")
}

fn sanitize_osc_text(raw: &str) -> String {
    raw.chars().filter(|ch| !ch.is_control()).collect()
}
