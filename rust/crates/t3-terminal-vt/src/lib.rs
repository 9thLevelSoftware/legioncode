#![forbid(unsafe_code)]
//! Pure Safe Rust Ghostty VT ABI mirror, streaming VT/OSC/paste safety engine, `"T3VT"` binary
//! snapshot codec, and sparse ANSI 256-color palette (`BUG-25`) for T3 Code.
//!
//! # Examples
//!
//! ```
//! use t3_terminal_vt::{
//!     AnsiPalette256, Rgba, TerminalGrid, decode_t3vt, encode_t3vt, sanitize_bracketed_paste,
//! };
//! use t3_terminal_vt::palette::xterm_256_rgb;
//!
//! // BUG-25: sparse theme overrides keep unset slots at `None` and reset on empty theme.
//! let mut palette = AnsiPalette256::new();
//! palette.apply_theme_overrides(&[(1, Rgba::rgb(255, 0, 0)), (196, Rgba::rgb(250, 10, 10))]);
//! assert_eq!(palette.override_at(0), None);
//! assert_eq!(palette.resolve_rgb(0), xterm_256_rgb(0));
//! palette.apply_theme_overrides(&[]);
//! assert_eq!(palette.override_at(1), None);
//! assert_eq!(palette.override_at(196), None);
//!
//! // Streaming VT parser + OSC title + T3VT snapshot round-trip.
//! let mut grid = TerminalGrid::new(20, 4);
//! grid.feed(b"\x1b]2;Shell\x07\x1b[32mhi\x1b[0m");
//! assert_eq!(grid.title(), "Shell");
//! let snap = grid.snapshot();
//! let encoded = encode_t3vt(&snap);
//! assert_eq!(decode_t3vt(&encoded), Ok(snap));
//!
//! // Bracketed paste sanitizer strips ESC[201~, unsafe C0 controls, and OSC injections.
//! let clean = sanitize_bracketed_paste("ok\t\n\x1b[201~evil\x1b]0;pwn\x07\x00!");
//! assert_eq!(clean, "ok\t\nevil!");
//! ```

mod abi_screen;
mod abi_terminal;
mod grid;
mod grid_edit;
mod grid_motion;
mod grid_ops;
mod grid_view;
mod parser_dispatch;
mod parser_osc;
mod parser_utf8;
mod sgr;
mod snapshot_wire;
mod theme_config;
mod unicode_width;
mod xterm256;

pub mod abi;
pub mod error;
pub mod palette;
pub mod parser;
pub mod paste;
pub mod snapshot;

pub use error::VtError;
pub use palette::{AnsiPalette256, Rgba};
pub use parser::{TerminalGrid, VtParser};
pub use paste::sanitize_bracketed_paste;
pub use snapshot::{SnapshotCell, TerminalSnapshot, decode_t3vt, encode_t3vt};

/// Number of indexed color slots in the `VT` terminal palette (`0..=255`).
pub const PALETTE_LEN: usize = 256;
