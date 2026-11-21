//! Error types for `t3-terminal-vt` snapshot decoding, grid validation, and color parsing.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors returned by `t3-terminal-vt` parsers, grid bounds checks, and `"T3VT"` snapshot codecs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Error)]
pub enum VtError {
    /// Terminal grid dimensions must be non-zero and within supported limits.
    #[error("invalid terminal dimensions {cols}x{rows}")]
    InvalidDimensions {
        /// Requested column count.
        cols: u16,
        /// Requested row count.
        rows: u16,
    },
    /// Binary snapshot buffer ended earlier than required by the header or payload.
    #[error("truncated T3VT snapshot: expected at least {expected_min} bytes, got {actual}")]
    TruncatedBuffer {
        /// Minimum byte count required at the point of failure.
        expected_min: usize,
        /// Actual byte count available.
        actual: usize,
    },
    /// Binary snapshot magic bytes did not match `b"T3VT"`.
    #[error("invalid T3VT magic header: {found:?}")]
    InvalidMagic {
        /// First four bytes found in the input buffer.
        found: [u8; 4],
    },
    /// Binary snapshot version is not supported.
    #[error("unsupported T3VT snapshot version {0}")]
    UnsupportedVersion(u16),
    /// Binary snapshot checksum did not match the computed integrity digest.
    #[error("T3VT checksum mismatch: expected {expected:#010x}, got {actual:#010x}")]
    ChecksumMismatch {
        /// Checksum encoded in the trailer.
        expected: u32,
        /// Checksum computed over the payload.
        actual: u32,
    },
    /// A boolean wire field contained a byte value other than `0` or `1`.
    #[error("invalid boolean value {value} for field '{field}'")]
    InvalidBooleanField {
        /// Field name in the `"T3VT"` header.
        field: &'static str,
        /// Raw byte encountered.
        value: u8,
    },
    /// Cursor shape discriminant was outside `0..=3`.
    #[error("invalid cursor style value {0}")]
    InvalidCursorStyle(u8),
    /// Palette override count exceeded `256`.
    #[error("invalid palette override count {0}: maximum is 256")]
    InvalidPaletteCount(u16),
    /// Snapshot palette table contained duplicate entries for the same index.
    #[error("duplicate palette override for index {0}")]
    DuplicatePaletteIndex(u8),
    /// `cols * rows` overflowed or exceeded the maximum snapshot cell limit.
    #[error("terminal snapshot cell count {cols}x{rows} exceeds maximum limit")]
    CellCountOverflow {
        /// Column count.
        cols: u16,
        /// Row count.
        rows: u16,
    },
    /// Snapshot `cells` length did not match `cols * rows`.
    #[error("snapshot cell count mismatch: expected {expected}, got {actual}")]
    CellCountMismatch {
        /// Expected `cols * rows`.
        expected: usize,
        /// Actual slice length.
        actual: usize,
    },
    /// Cell text payload contained invalid UTF-8 bytes.
    #[error("invalid UTF-8 sequence in T3VT cell text")]
    InvalidUtf8,
    /// Snapshot buffer contained unexpected trailing bytes after decoding all fields.
    #[error("unexpected {0} trailing bytes in T3VT snapshot")]
    TrailingBytes(usize),
    /// Color string could not be parsed as `#RGB`, `#RRGGBB`, `#RRGGBBAA`, or `rgb:R/G/B`.
    #[error("invalid terminal color specification '{0}'")]
    InvalidColor(String),
}
