//! UTF-16 / UTF-8 offset translation, surrogate-safe slicing, and 1:1 UTF-16 folding.
//!
//! Matches the TypeScript UTF-16 span contract in `packages/shared/src/{paletteSearch.ts,
//! assistantCitations.ts,threadSearch.ts}` where UI highlights, citations, and search offsets
//! count UTF-16 code units while server storage and socket budgets operate on UTF-8 bytes.

use crate::utf16_fold_table::FOLD_OVERRIDES;

/// Errors produced when translating or slicing UTF-16 code-unit offsets against UTF-8 text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Utf16OffsetError {
    /// The requested UTF-16 code-unit offset lands in the middle of a surrogate pair.
    #[error("UTF-16 offset {offset} splits a surrogate pair")]
    SurrogateSplit {
        /// The splitting UTF-16 offset.
        offset: usize,
    },
    /// The requested UTF-16 code-unit offset exceeds the string's UTF-16 length.
    #[error("UTF-16 offset {offset} is out of bounds for UTF-16 length {utf16_len}")]
    OutOfBounds {
        /// The out-of-bounds UTF-16 offset.
        offset: usize,
        /// The total UTF-16 code-unit length of the string.
        utf16_len: usize,
    },
    /// The supplied UTF-8 byte offset does not fall on a UTF-8 scalar boundary.
    #[error("UTF-8 byte offset {offset} is not a char boundary for byte length {utf8_len}")]
    InvalidUtf8Boundary {
        /// The invalid UTF-8 byte offset.
        offset: usize,
        /// The total UTF-8 byte length of the string.
        utf8_len: usize,
    },
    /// The start offset exceeds the end offset in a strict slice request.
    #[error("UTF-16 range {start}..{end} is inverted (start > end)")]
    InvertedRange {
        /// Requested start UTF-16 offset.
        start: usize,
        /// Requested end UTF-16 offset.
        end: usize,
    },
}

/// Returns `true` when `unit` is a UTF-16 high (leading) surrogate (`0xD800..=0xDBFF`).
#[inline]
pub const fn is_high_surrogate(unit: u16) -> bool {
    matches!(unit, 0xD800..=0xDBFF)
}

/// Returns `true` when `unit` is a UTF-16 low (trailing) surrogate (`0xDC00..=0xDFFF`).
#[inline]
pub const fn is_low_surrogate(unit: u16) -> bool {
    matches!(unit, 0xDC00..=0xDFFF)
}

/// Returns `true` when `offset` falls between a high and low surrogate in `units`.
///
/// Matches `splitsSurrogatePair` from `assistantCitations.ts`.
#[inline]
pub fn splits_surrogate_pair(units: &[u16], offset: usize) -> bool {
    if offset == 0 || offset >= units.len() {
        return false;
    }
    is_high_surrogate(units[offset - 1]) && is_low_surrogate(units[offset])
}

/// Returns the number of UTF-16 code units needed to represent `text`.
///
/// # Examples
///
/// ```
/// use t3_shared::utf16::utf8_len_to_utf16;
///
/// assert_eq!(utf8_len_to_utf16("hello"), 5);
/// assert_eq!(utf8_len_to_utf16("a😀b"), 4);
/// assert_eq!(utf8_len_to_utf16("café"), 4);
/// ```
#[inline]
pub fn utf8_len_to_utf16(text: &str) -> usize {
    if text.is_ascii() {
        return text.len();
    }
    text.chars().map(char::len_utf16).sum()
}

/// Converts a UTF-8 byte offset inside `text` to its corresponding UTF-16 code-unit offset.
///
/// Returns `Err(Utf16OffsetError::InvalidUtf8Boundary)` if `utf8_byte` is past the end of `text`
/// or lands inside a multi-byte UTF-8 sequence.
///
/// # Examples
///
/// ```
/// use t3_shared::utf16::utf8_byte_to_utf16_offset;
///
/// let text = "a😀b";
/// assert_eq!(utf8_byte_to_utf16_offset(text, 1), Ok(1));
/// assert_eq!(utf8_byte_to_utf16_offset(text, 5), Ok(3));
/// assert!(utf8_byte_to_utf16_offset(text, 2).is_err());
/// ```
pub fn utf8_byte_to_utf16_offset(text: &str, utf8_byte: usize) -> Result<usize, Utf16OffsetError> {
    if !text.is_char_boundary(utf8_byte) {
        return Err(Utf16OffsetError::InvalidUtf8Boundary {
            offset: utf8_byte,
            utf8_len: text.len(),
        });
    }
    Ok(utf8_len_to_utf16(&text[..utf8_byte]))
}

/// Converts a UTF-16 code-unit offset inside `text` to the corresponding UTF-8 byte offset.
///
/// Returns `Err(Utf16OffsetError::SurrogateSplit)` when `utf16_offset` lands inside a surrogate
/// pair (e.g. offset `2` inside `"a😀b"`), and `Err(Utf16OffsetError::OutOfBounds)` when
/// `utf16_offset` exceeds `utf8_len_to_utf16(text)`.
///
/// # Examples
///
/// ```
/// use t3_shared::utf16::{Utf16OffsetError, utf16_offset_to_utf8_byte};
///
/// let text = "a😀b";
/// assert_eq!(utf16_offset_to_utf8_byte(text, 0), Ok(0));
/// assert_eq!(utf16_offset_to_utf8_byte(text, 1), Ok(1));
/// assert_eq!(
///     utf16_offset_to_utf8_byte(text, 2),
///     Err(Utf16OffsetError::SurrogateSplit { offset: 2 })
/// );
/// assert_eq!(utf16_offset_to_utf8_byte(text, 3), Ok(5));
/// assert_eq!(utf16_offset_to_utf8_byte(text, 4), Ok(6));
/// ```
pub fn utf16_offset_to_utf8_byte(
    text: &str,
    utf16_offset: usize,
) -> Result<usize, Utf16OffsetError> {
    if text.is_ascii() {
        return if utf16_offset <= text.len() {
            Ok(utf16_offset)
        } else {
            Err(Utf16OffsetError::OutOfBounds {
                offset: utf16_offset,
                utf16_len: text.len(),
            })
        };
    }

    let mut cur_utf16 = 0usize;
    for (byte_idx, ch) in text.char_indices() {
        if cur_utf16 == utf16_offset {
            return Ok(byte_idx);
        }
        let width = ch.len_utf16();
        if cur_utf16 + width > utf16_offset {
            return Err(Utf16OffsetError::SurrogateSplit {
                offset: utf16_offset,
            });
        }
        cur_utf16 += width;
    }

    if cur_utf16 == utf16_offset {
        Ok(text.len())
    } else {
        Err(Utf16OffsetError::OutOfBounds {
            offset: utf16_offset,
            utf16_len: cur_utf16,
        })
    }
}

/// Safely slices `text` by UTF-16 `[start_utf16..end_utf16)` bounds without panicking or
/// splitting surrogate pairs.
///
/// Out-of-bounds endpoints clamp to the string length. If `start_utf16` splits a surrogate pair,
/// it snaps forward past that character; if `end_utf16` splits a surrogate pair, it snaps
/// backward before that character (matching `assistantCitations.ts` context boundaries).
/// Inverted or empty ranges return `""`.
///
/// # Examples
///
/// ```
/// use t3_shared::utf16::utf16_slice_safe;
///
/// let text = "a😀b";
/// assert_eq!(utf16_slice_safe(text, 0, 4), "a😀b");
/// assert_eq!(utf16_slice_safe(text, 0, 2), "a");
/// assert_eq!(utf16_slice_safe(text, 2, 4), "b");
/// assert_eq!(utf16_slice_safe(text, 1, 2), "");
/// assert_eq!(utf16_slice_safe(text, 3, 1), "");
/// ```
pub fn utf16_slice_safe(text: &str, start_utf16: usize, end_utf16: usize) -> &str {
    if start_utf16 >= end_utf16 || text.is_empty() {
        return "";
    }
    if text.is_ascii() {
        let start = start_utf16.min(text.len());
        let end = end_utf16.min(text.len());
        return if start >= end { "" } else { &text[start..end] };
    }

    let mut cur_utf16 = 0usize;
    let mut start_byte = None;
    let mut end_byte = 0usize;

    for (byte_idx, ch) in text.char_indices() {
        if start_byte.is_none() && cur_utf16 >= start_utf16 {
            start_byte = Some(byte_idx);
        }
        if cur_utf16 <= end_utf16 {
            end_byte = byte_idx;
        }
        let next_utf16 = cur_utf16 + ch.len_utf16();
        if next_utf16 <= end_utf16 {
            end_byte = byte_idx + ch.len_utf8();
        } else {
            break;
        }
        cur_utf16 = next_utf16;
    }

    let Some(start) = start_byte else {
        return "";
    };
    if start >= end_byte {
        ""
    } else {
        &text[start..end_byte]
    }
}

/// Strictly slices `text` by exact UTF-16 `[start_utf16..end_utf16)` offsets.
///
/// Fails if either endpoint splits a surrogate pair, is out of bounds, or `start > end`.
///
/// # Examples
///
/// ```
/// use t3_shared::utf16::{Utf16OffsetError, utf16_slice_exact};
///
/// assert_eq!(utf16_slice_exact("a😀b", 1, 3), Ok("😀"));
/// assert_eq!(
///     utf16_slice_exact("a😀b", 0, 2),
///     Err(Utf16OffsetError::SurrogateSplit { offset: 2 })
/// );
/// ```
pub fn utf16_slice_exact(
    text: &str,
    start_utf16: usize,
    end_utf16: usize,
) -> Result<&str, Utf16OffsetError> {
    if start_utf16 > end_utf16 {
        return Err(Utf16OffsetError::InvertedRange {
            start: start_utf16,
            end: end_utf16,
        });
    }
    let start_byte = utf16_offset_to_utf8_byte(text, start_utf16)?;
    let end_byte = utf16_offset_to_utf8_byte(text, end_utf16)?;
    Ok(&text[start_byte..end_byte])
}

/// Folds a single Unicode scalar using the 1:1 UTF-16 length-preserving rule of `paletteSearch.ts`.
///
/// Case and Latin/Greek/compatibility accents fold to lowercase without changing UTF-16 length:
/// - `'İ'` (`U+0130`, whose `.to_lowercase()` expands to `"i\u{0307}"`) folds to `'i'`.
/// - `'ß'` (`U+00DF`) and `'ﬁ'` (`U+FB01`) stay unchanged (`'ß'`, `'ﬁ'`).
/// - Final sigma `'ς'` folds to `'σ'`, and curly apostrophes `'‘'`/`'’'` fold to `'\''`.
///
/// # Examples
///
/// ```
/// use t3_shared::utf16::fold_char_in_place;
///
/// assert_eq!(fold_char_in_place('É'), 'e');
/// assert_eq!(fold_char_in_place('İ'), 'i');
/// assert_eq!(fold_char_in_place('ß'), 'ß');
/// assert_eq!(fold_char_in_place('ﬁ'), 'ﬁ');
/// assert_eq!(fold_char_in_place('ς'), 'σ');
/// assert_eq!(fold_char_in_place('’'), '\'');
/// ```
#[inline]
pub fn fold_char_in_place(ch: char) -> char {
    if ch.is_ascii() {
        return ch.to_ascii_lowercase();
    }
    let code = ch as u32;
    if let Ok(unit) = u16::try_from(code)
        && let Ok(idx) = FOLD_OVERRIDES.binary_search_by_key(&unit, |&entry| (entry >> 16) as u16)
    {
        let mapped = FOLD_OVERRIDES[idx] & 0xFFFF;
        return char::from_u32(mapped).unwrap_or(ch);
    }
    ch.to_lowercase().next().unwrap_or(ch)
}

/// Folds `text` 1:1 in place so every character keeps its exact UTF-16 code-unit length.
///
/// Because `utf8_len_to_utf16(&fold_in_place(text)) == utf8_len_to_utf16(text)` always holds,
/// UTF-16 highlight spans computed on folded text map directly onto `text` with no offset table.
///
/// # Examples
///
/// ```
/// use t3_shared::utf16::{fold_in_place, utf8_len_to_utf16};
///
/// let input = "Café İstanbul ß ﬁ Don’t 🚀";
/// let folded = fold_in_place(input);
/// assert_eq!(folded, "cafe istanbul ß ﬁ don't 🚀");
/// assert_eq!(utf8_len_to_utf16(&folded), utf8_len_to_utf16(input));
/// ```
pub fn fold_in_place(text: &str) -> String {
    if text.is_ascii() {
        return text.to_ascii_lowercase();
    }
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        out.push(fold_char_in_place(ch));
    }
    out
}

/// Folds `text` 1:1 in place directly into UTF-16 code units.
///
/// Used by `FuzzyPaletteIndex` to build contiguous `SoA` UTF-16 buffers without intermediate strings.
///
/// # Examples
///
/// ```
/// use t3_shared::utf16::{fold_in_place_utf16, utf8_len_to_utf16};
///
/// let input = "İçerik 🚀";
/// let folded = fold_in_place_utf16(input);
/// assert_eq!(folded.len(), utf8_len_to_utf16(input));
/// assert_eq!(String::from_utf16_lossy(&folded), "icerik 🚀");
/// ```
pub fn fold_in_place_utf16(text: &str) -> Vec<u16> {
    let mut out = Vec::with_capacity(utf8_len_to_utf16(text));
    if text.is_ascii() {
        for byte in text.bytes() {
            out.push(u16::from(byte.to_ascii_lowercase()));
        }
        return out;
    }
    let mut buf = [0u16; 2];
    for ch in text.chars() {
        let folded = fold_char_in_place(ch);
        out.extend_from_slice(folded.encode_utf16(&mut buf));
    }
    out
}
