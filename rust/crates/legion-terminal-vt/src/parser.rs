//! Streaming UTF-8 + `CSI`/`SGR`/`OSC` VT parser and bounded [`TerminalGrid`].

pub use crate::grid::{DEFAULT_MAX_SCROLLBACK, MAX_GRID_COLS, MAX_GRID_ROWS, TerminalGrid};
pub use crate::grid_ops::{GridCell, GridRow, MAX_CELL_GRAPHEME_BYTES};
pub use crate::sgr::{CellAttributes, CellColor, apply_sgr};

use crate::parser_dispatch::{dispatch_csi, dispatch_esc};
use crate::parser_osc::dispatch_osc;
use crate::parser_utf8::Utf8Decoder;

const MAX_CSI_PARAMS: usize = 32;
const MAX_INTERMEDIATES: usize = 4;
const MAX_OSC_BYTES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ParserState {
    #[default]
    Ground,
    Escape,
    EscapeIntermediate,
    CsiEntry,
    CsiParam,
    CsiIgnore,
    OscString,
    OscEsc,
    StringIgnore,
    StringIgnoreEsc,
}

/// Streaming byte-by-byte VT and UTF-8 parser driving a [`TerminalGrid`].
#[derive(Debug, Clone, Default)]
pub struct VtParser {
    state: ParserState,
    utf8: Utf8Decoder,
    intermediates: Vec<u8>,
    csi_private: Option<u8>,
    csi_params: Vec<(u16, bool)>,
    csi_current: u16,
    csi_param_started: bool,
    csi_next_colon: bool,
    osc_buf: Vec<u8>,
}

impl VtParser {
    /// Creates a new parser initialized in `Ground` state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Resets any partial UTF-8 sequence and escape state back to `Ground`.
    pub fn reset(&mut self) {
        self.state = ParserState::Ground;
        self.utf8.reset();
        self.intermediates.clear();
        self.csi_private = None;
        self.csi_params.clear();
        self.csi_current = 0;
        self.csi_param_started = false;
        self.csi_next_colon = false;
        self.osc_buf.clear();
    }

    /// Feeds an arbitrary chunk of PTY bytes, preserving split UTF-8 and escape sequences across
    /// calls.
    pub fn feed(&mut self, bytes: &[u8], grid: &mut TerminalGrid) {
        for &byte in bytes {
            self.step_byte(byte, grid);
        }
    }

    fn step_byte(&mut self, byte: u8, grid: &mut TerminalGrid) {
        let (interrupted, decoded, ascii) = self.utf8.push(byte);
        if let Some(ch) = interrupted {
            self.emit_decoded_char(ch, grid);
        }
        if let Some(ch) = decoded {
            self.emit_decoded_char(ch, grid);
        }
        if let Some(ascii_byte) = ascii {
            self.step_ascii(ascii_byte, grid);
        }
    }

    fn emit_decoded_char(&mut self, ch: char, grid: &mut TerminalGrid) {
        match self.state {
            ParserState::Ground => grid.put_char(ch),
            ParserState::OscString => {
                let mut tmp = [0_u8; 4];
                let encoded = ch.encode_utf8(&mut tmp);
                self.push_osc_bytes(encoded.as_bytes());
            }
            ParserState::OscEsc => {
                self.state = ParserState::OscString;
                let mut tmp = [0_u8; 4];
                let encoded = ch.encode_utf8(&mut tmp);
                self.push_osc_bytes(encoded.as_bytes());
            }
            _ => {}
        }
    }

    fn step_ascii(&mut self, byte: u8, grid: &mut TerminalGrid) {
        match self.state {
            ParserState::Ground => self.handle_ground_ascii(byte, grid),
            ParserState::Escape => self.handle_escape(byte, grid),
            ParserState::EscapeIntermediate => self.handle_escape_intermediate(byte, grid),
            ParserState::CsiEntry => self.handle_csi_entry(byte, grid),
            ParserState::CsiParam => self.handle_csi_param(byte, grid),
            ParserState::CsiIgnore => self.handle_csi_ignore(byte, grid),
            ParserState::OscString => self.handle_osc_string(byte, grid),
            ParserState::OscEsc => self.handle_osc_esc(byte, grid),
            ParserState::StringIgnore => {
                if byte == 0x1B {
                    self.state = ParserState::StringIgnoreEsc;
                } else if byte == 0x07 {
                    self.state = ParserState::Ground;
                }
            }
            ParserState::StringIgnoreEsc => {
                if byte == b'\\' {
                    self.state = ParserState::Ground;
                } else if byte == 0x1B {
                    self.state = ParserState::StringIgnoreEsc;
                } else {
                    self.state = ParserState::StringIgnore;
                }
            }
        }
    }

    fn handle_ground_ascii(&mut self, byte: u8, grid: &mut TerminalGrid) {
        match byte {
            0x07 => grid.bell_count = grid.bell_count.saturating_add(1),
            0x08 => grid.backspace(),
            0x09 => grid.tab(1),
            0x0A..=0x0C => grid.line_feed(),
            0x0D => grid.carriage_return(),
            0x1B => self.enter_escape(),
            0x20..=0x7E => grid.put_char(char::from(byte)),
            _ => {}
        }
    }

    fn handle_escape(&mut self, byte: u8, grid: &mut TerminalGrid) {
        match byte {
            0x18 | 0x1A => self.state = ParserState::Ground,
            0x1B => self.enter_escape(),
            0x20..=0x2F => {
                self.push_intermediate(byte);
                self.state = ParserState::EscapeIntermediate;
            }
            b'[' => self.enter_csi(),
            b']' => self.enter_osc(),
            b'P' | b'X' | b'^' | b'_' => self.state = ParserState::StringIgnore,
            0x30..=0x7E => {
                dispatch_esc(grid, &self.intermediates, byte);
                self.state = ParserState::Ground;
            }
            _ => Self::handle_c0_inside_seq(byte, grid),
        }
    }

    fn handle_escape_intermediate(&mut self, byte: u8, grid: &mut TerminalGrid) {
        match byte {
            0x18 | 0x1A => self.state = ParserState::Ground,
            0x1B => self.enter_escape(),
            0x20..=0x2F => self.push_intermediate(byte),
            0x30..=0x7E => {
                dispatch_esc(grid, &self.intermediates, byte);
                self.state = ParserState::Ground;
            }
            _ => Self::handle_c0_inside_seq(byte, grid),
        }
    }

    fn handle_csi_entry(&mut self, byte: u8, grid: &mut TerminalGrid) {
        match byte {
            0x18 | 0x1A => self.state = ParserState::Ground,
            0x1B => self.enter_escape(),
            b'<'..=b'?' => {
                self.csi_private = Some(byte);
                self.state = ParserState::CsiParam;
            }
            b'0'..=b'9' => {
                self.csi_param_started = true;
                self.csi_current = u16::from(byte - b'0');
                self.state = ParserState::CsiParam;
            }
            b';' | b':' => {
                self.push_csi_param(0);
                self.csi_next_colon = byte == b':';
                self.state = ParserState::CsiParam;
            }
            0x20..=0x2F => {
                self.push_intermediate(byte);
                self.state = ParserState::CsiParam;
            }
            0x40..=0x7E => {
                dispatch_csi(
                    grid,
                    self.csi_private,
                    &self.intermediates,
                    &self.csi_params,
                    byte,
                );
                self.state = ParserState::Ground;
            }
            _ => Self::handle_c0_inside_seq(byte, grid),
        }
    }

    fn handle_csi_param(&mut self, byte: u8, grid: &mut TerminalGrid) {
        match byte {
            0x18 | 0x1A => self.state = ParserState::Ground,
            0x1B => self.enter_escape(),
            b'0'..=b'9' => {
                self.csi_param_started = true;
                self.csi_current = self
                    .csi_current
                    .saturating_mul(10)
                    .saturating_add(u16::from(byte - b'0'));
            }
            b';' | b':' => {
                let value = if self.csi_param_started {
                    self.csi_current
                } else {
                    0
                };
                self.push_csi_param(value);
                self.csi_current = 0;
                self.csi_param_started = false;
                self.csi_next_colon = byte == b':';
            }
            0x20..=0x2F => self.push_intermediate(byte),
            b'<'..=b'?' => self.state = ParserState::CsiIgnore,
            0x40..=0x7E => {
                if self.csi_param_started || !self.csi_params.is_empty() {
                    let value = if self.csi_param_started {
                        self.csi_current
                    } else {
                        0
                    };
                    self.push_csi_param(value);
                }
                dispatch_csi(
                    grid,
                    self.csi_private,
                    &self.intermediates,
                    &self.csi_params,
                    byte,
                );
                self.state = ParserState::Ground;
            }
            _ => Self::handle_c0_inside_seq(byte, grid),
        }
    }

    fn handle_csi_ignore(&mut self, byte: u8, grid: &mut TerminalGrid) {
        match byte {
            0x18 | 0x1A | 0x40..=0x7E => self.state = ParserState::Ground,
            0x1B => self.enter_escape(),
            _ => Self::handle_c0_inside_seq(byte, grid),
        }
    }

    fn handle_osc_string(&mut self, byte: u8, grid: &mut TerminalGrid) {
        match byte {
            0x07 => {
                dispatch_osc(grid, &self.osc_buf);
                self.osc_buf.clear();
                self.state = ParserState::Ground;
            }
            0x1B => self.state = ParserState::OscEsc,
            _ => self.push_osc_bytes(&[byte]),
        }
    }

    fn handle_osc_esc(&mut self, byte: u8, grid: &mut TerminalGrid) {
        if byte == b'\\' {
            dispatch_osc(grid, &self.osc_buf);
            self.osc_buf.clear();
            self.state = ParserState::Ground;
            return;
        }
        // Unterminated OSC aborted by a fresh escape sequence (`ESC [...]` etc.).
        self.osc_buf.clear();
        self.enter_escape();
        self.handle_escape(byte, grid);
    }

    fn handle_c0_inside_seq(byte: u8, grid: &mut TerminalGrid) {
        match byte {
            0x07 => grid.bell_count = grid.bell_count.saturating_add(1),
            0x08 => grid.backspace(),
            0x09 => grid.tab(1),
            0x0A..=0x0C => grid.line_feed(),
            0x0D => grid.carriage_return(),
            _ => {}
        }
    }

    fn enter_escape(&mut self) {
        self.intermediates.clear();
        self.state = ParserState::Escape;
    }

    fn enter_csi(&mut self) {
        self.intermediates.clear();
        self.csi_private = None;
        self.csi_params.clear();
        self.csi_current = 0;
        self.csi_param_started = false;
        self.csi_next_colon = false;
        self.state = ParserState::CsiEntry;
    }

    fn enter_osc(&mut self) {
        self.osc_buf.clear();
        self.state = ParserState::OscString;
    }

    fn push_intermediate(&mut self, byte: u8) {
        if self.intermediates.len() < MAX_INTERMEDIATES {
            self.intermediates.push(byte);
        }
    }

    fn push_csi_param(&mut self, value: u16) {
        if self.csi_params.len() < MAX_CSI_PARAMS {
            self.csi_params.push((value, self.csi_next_colon));
        } else {
            self.state = ParserState::CsiIgnore;
        }
    }

    fn push_osc_bytes(&mut self, bytes: &[u8]) {
        let remaining = MAX_OSC_BYTES.saturating_sub(self.osc_buf.len());
        let take = bytes.len().min(remaining);
        self.osc_buf.extend_from_slice(&bytes[..take]);
    }
}
