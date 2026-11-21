//! WebSocket large-frame splitting and reassembly (`?frameParts`).
//!
//! Mirrors `packages/shared/src/socketFrameParts.ts`: frames larger than
//! [`SOCKET_FRAME_PART_BYTES`] (48 KiB UTF-8 on the wire) are split into `\u{001e}`-prefixed
//! parts at valid UTF-8 scalar boundaries, while [`FramePartAssembler`] emits synthetic
//! [`PART_RECEIVED_FRAME`] (`{"_tag":"Pong"}`) acknowledgements for each intermediate part so
//! client liveness timers do not expire mid-transfer.
//!
//! Unlike the legacy UTF-16 code-unit byte estimator (BUG-21), splitting operates directly on
//! UTF-8 scalar boundaries (and decodes raw UTF-16 via [`std::char::decode_utf16`]), so an
//! unpaired high surrogate followed by a 3-byte BMP character cannot over-run `part_bytes`.

use std::borrow::Cow;
use std::char::decode_utf16;

/// Query parameter enabled by clients that understand split WebSocket text frames.
pub const SOCKET_FRAME_PARTS_QUERY_PARAM: &str = "frameParts";

/// Maximum UTF-8 byte length of a single WebSocket text frame part (`48 KiB`).
pub const SOCKET_FRAME_PART_BYTES: usize = 48 * 1024;

/// Prefix prepended to every frame part except the final part (`U+001E` Record Separator).
pub const PART_PREFIX: char = '\u{001e}';

/// Synthetic pong message emitted by [`FramePartAssembler`] for each buffered intermediate part.
pub const PART_RECEIVED_FRAME: &str = r#"{"_tag":"Pong"}"#;

/// Minimum `part_bytes` limit guaranteed to fit [`PART_PREFIX`] plus any 4-byte UTF-8 scalar.
pub const MIN_SOCKET_FRAME_PART_BYTES: usize = PART_PREFIX.len_utf8() + 4;

/// Errors returned when splitting a frame with a custom byte limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FramePartsError {
    /// The caller-supplied `part_bytes` limit cannot fit [`PART_PREFIX`] and the next character.
    #[error("socket frame part limit {part_bytes} bytes is below required minimum {minimum} bytes")]
    PartLimitTooSmall {
        /// Requested maximum bytes per part.
        part_bytes: usize,
        /// Minimum bytes required to encode [`PART_PREFIX`] plus the next scalar.
        minimum: usize,
    },
}

/// Result of feeding a single incoming WebSocket text frame into [`FramePartAssembler`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameOutput<'a> {
    /// Either an unsplit frame or the completed reassembly of a multi-part frame.
    Complete(Cow<'a, str>),
    /// An intermediate `\u{001e}`-prefixed part was buffered; forward this synthetic pong ack.
    IntermediateAck(&'static str),
}

impl FrameOutput<'_> {
    /// Returns the wire frame string to pass downstream (the complete frame or synthetic pong).
    #[inline]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Complete(frame) => frame.as_ref(),
            Self::IntermediateAck(ack) => ack,
        }
    }

    /// Consumes this outcome into an owned `String` (cloned only when borrowed).
    #[inline]
    pub fn into_owned(self) -> String {
        match self {
            Self::Complete(frame) => frame.into_owned(),
            Self::IntermediateAck(ack) => ack.to_owned(),
        }
    }
}

/// Client-side reassembler joining `\u{001e}`-prefixed parts back into whole WebSocket frames.
///
/// # Examples
///
/// ```
/// use t3_shared::frame_parts::{
///     FrameOutput, FramePartAssembler, PART_RECEIVED_FRAME, split_socket_frame_with_limit,
/// };
///
/// let payload = "hello ".repeat(20);
/// let parts = split_socket_frame_with_limit(&payload, 32).expect("valid limit");
/// assert!(parts.len() > 1);
///
/// let mut assembler = FramePartAssembler::new();
/// let mut assembled = None;
/// for part in &parts {
///     match assembler.push(part) {
///         FrameOutput::IntermediateAck(ack) => assert_eq!(ack, PART_RECEIVED_FRAME),
///         FrameOutput::Complete(whole) => assembled = Some(whole.into_owned()),
///     }
/// }
/// assert_eq!(assembled.as_deref(), Some(payload.as_str()));
/// assert!(!assembler.is_pending());
/// ```
#[derive(Debug, Default, Clone)]
pub struct FramePartAssembler {
    pending: String,
}

impl FramePartAssembler {
    /// Creates an empty [`FramePartAssembler`].
    #[inline]
    pub fn new() -> Self {
        Self {
            pending: String::new(),
        }
    }

    /// Feeds one incoming WebSocket text frame into the assembler.
    ///
    /// - If `frame` starts with [`PART_PREFIX`], appends the slice after the prefix and returns
    ///   [`FrameOutput::IntermediateAck`] carrying [`PART_RECEIVED_FRAME`].
    /// - Otherwise, if parts were buffered, appends `frame` as the final part, resets internal
    ///   state, and returns [`FrameOutput::Complete`] with the joined string.
    /// - Otherwise returns [`FrameOutput::Complete`] borrowing `frame` without allocation.
    pub fn push<'a>(&mut self, frame: &'a str) -> FrameOutput<'a> {
        if let Some(chunk) = frame.strip_prefix(PART_PREFIX) {
            self.pending.push_str(chunk);
            return FrameOutput::IntermediateAck(PART_RECEIVED_FRAME);
        }
        if self.pending.is_empty() {
            return FrameOutput::Complete(Cow::Borrowed(frame));
        }
        self.pending.push_str(frame);
        FrameOutput::Complete(Cow::Owned(std::mem::take(&mut self.pending)))
    }

    /// Returns `true` when one or more intermediate parts are buffered awaiting the final part.
    #[inline]
    pub fn is_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Returns the buffered UTF-8 byte count currently waiting for a terminating frame.
    #[inline]
    pub fn pending_bytes(&self) -> usize {
        self.pending.len()
    }

    /// Discards any buffered incomplete parts.
    #[inline]
    pub fn reset(&mut self) {
        self.pending.clear();
    }
}

/// Splits `frame` into WebSocket text parts bounded by [`SOCKET_FRAME_PART_BYTES`] (`48 KiB`).
///
/// Always returns at least one part, never splits inside a multi-byte UTF-8 character, and
/// prefixes every part except the final one with [`PART_PREFIX`].
///
/// # Examples
///
/// ```
/// use t3_shared::frame_parts::{SOCKET_FRAME_PART_BYTES, split_socket_frame};
///
/// let small = "{\"_tag\":\"Ping\"}";
/// assert_eq!(split_socket_frame(small), vec![small.to_owned()]);
///
/// let large = "🚀".repeat(15_000);
/// let parts = split_socket_frame(&large);
/// assert!(parts.len() > 1);
/// assert!(parts.iter().all(|part| part.len() <= SOCKET_FRAME_PART_BYTES));
/// ```
pub fn split_socket_frame(frame: &str) -> Vec<String> {
    // `SOCKET_FRAME_PART_BYTES` (49_152) exceeds `MIN_SOCKET_FRAME_PART_BYTES` (5),
    // so splitting cannot fail; `unwrap_or_else` keeps this infallibly panic-free.
    split_socket_frame_with_limit(frame, SOCKET_FRAME_PART_BYTES)
        .unwrap_or_else(|_| vec![frame.to_owned()])
}

/// Splits `frame` into WebSocket text parts bounded by `part_bytes` UTF-8 bytes on the wire.
///
/// Every intermediate part starts with [`PART_PREFIX`] and has UTF-8 byte length `<= part_bytes`.
/// The final part omits [`PART_PREFIX`] and also has UTF-8 byte length `<= part_bytes`.
///
/// # Examples
///
/// ```
/// use t3_shared::frame_parts::{PART_PREFIX, split_socket_frame_with_limit};
///
/// let frame = "😀😀😀";
/// let parts = split_socket_frame_with_limit(frame, 6).expect("6 >= 5 bytes");
/// assert_eq!(parts, vec![format!("{PART_PREFIX}😀"), format!("{PART_PREFIX}😀"), "😀".to_owned()]);
/// assert!(parts.iter().all(|part| part.len() <= 6));
/// ```
pub fn split_socket_frame_with_limit(
    frame: &str,
    part_bytes: usize,
) -> Result<Vec<String>, FramePartsError> {
    if frame.len() <= part_bytes {
        return Ok(vec![frame.to_owned()]);
    }
    let prefix_bytes = PART_PREFIX.len_utf8();
    if part_bytes <= prefix_bytes {
        let first_len = frame.chars().next().map_or(1, char::len_utf8);
        return Err(FramePartsError::PartLimitTooSmall {
            part_bytes,
            minimum: prefix_bytes + first_len,
        });
    }

    let max_payload = part_bytes - prefix_bytes;
    let mut parts = Vec::new();
    let mut start = 0usize;

    while frame.len() - start > part_bytes {
        let mut end = (start + max_payload).min(frame.len());
        while end > start && !frame.is_char_boundary(end) {
            end -= 1;
        }
        if end == start {
            let next_len = frame[start..].chars().next().map_or(1, char::len_utf8);
            return Err(FramePartsError::PartLimitTooSmall {
                part_bytes,
                minimum: prefix_bytes + next_len,
            });
        }

        let mut part = String::with_capacity(prefix_bytes + (end - start));
        part.push(PART_PREFIX);
        part.push_str(&frame[start..end]);
        parts.push(part);
        start = end;
    }

    parts.push(frame[start..].to_owned());
    Ok(parts)
}

/// Computes the exact UTF-8 wire byte count of raw UTF-16 `units` (substituting U+FFFD for
/// unpaired surrogates, fixing BUG-21's 4-byte paired over-count).
///
/// # Examples
///
/// ```
/// use t3_shared::frame_parts::utf16_wire_utf8_len;
///
/// // Unpaired high surrogate (0xD800) followed by '中' (0x4E2D):
/// // U+FFFD (3 bytes) + '中' (3 bytes) = 6 UTF-8 bytes, not 4.
/// assert_eq!(utf16_wire_utf8_len(&[0xD800, 0x4E2D]), 6);
/// // Valid surrogate pair '😀' (0xD83D, 0xDE00) = 4 UTF-8 bytes.
/// assert_eq!(utf16_wire_utf8_len(&[0xD83D, 0xDE00]), 4);
/// ```
pub fn utf16_wire_utf8_len(units: &[u16]) -> usize {
    decode_utf16(units.iter().copied())
        .map(|res| res.unwrap_or(char::REPLACEMENT_CHARACTER).len_utf8())
        .sum()
}

/// Decodes raw UTF-16 `units` (replacing unpaired surrogates with U+FFFD) and splits the
/// resulting UTF-8 string into parts bounded by `part_bytes` wire bytes.
///
/// Fixes BUG-21: an unpaired high surrogate followed by a 3-byte BMP character accounts for
/// `3 + 3 = 6` UTF-8 bytes rather than miscounting as a 4-byte pair.
///
/// # Examples
///
/// ```
/// use t3_shared::frame_parts::split_socket_frame_utf16;
///
/// let units = [0xD800u16, 0x4E2Du16, 0x4E2Du16]; // "\u{FFFD}中中" = 9 UTF-8 bytes
/// let parts = split_socket_frame_utf16(&units, 5).expect("5 bytes fits prefix + 3-byte char");
/// assert!(parts.iter().all(|part| part.len() <= 5));
/// assert_eq!(parts.len(), 3);
/// ```
pub fn split_socket_frame_utf16(
    units: &[u16],
    part_bytes: usize,
) -> Result<Vec<String>, FramePartsError> {
    let mut wire = String::with_capacity(units.len());
    for ch in decode_utf16(units.iter().copied()) {
        wire.push(ch.unwrap_or(char::REPLACEMENT_CHARACTER));
    }
    split_socket_frame_with_limit(&wire, part_bytes)
}
