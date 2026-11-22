//! Structure-of-Arrays (`SoA`) line, word-diff, and syntax-token buffers for diff rendering.
//!
//! Eliminates per-line and per-token heap `String`/`Vec` allocations across large diffs and
//! preserves `font_style` flags (bold/italic/underline) uniformly across web (`wasm32`) and
//! mobile (`UniFFI`) surfaces.

use serde::{Deserialize, Serialize};

/// Diff line classification inside a hunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiffLineKind {
    /// Unchanged context line (`' '`).
    Context,
    /// Added line (`'+'`).
    Add,
    /// Deleted line (`'-'`).
    Delete,
    /// Unified diff hunk header (`@@ ... @@`).
    HunkHeader,
}

/// Byte span `[start, start + len)` referencing [`DiffLineArrays`]'s text arena.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TextRange {
    /// Inclusive UTF-8 byte start offset in the arena.
    pub start: u32,
    /// Byte length of the slice.
    pub len: u32,
}

/// Slice window `[offset, offset + len)` into a packed parallel `SoA` array.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SliceRange {
    /// Starting element index in the packed child array.
    pub offset: u32,
    /// Number of elements belonging to the parent line.
    pub len: u32,
}

/// Half-open UTF-16 code-unit span `[start, end)` for intra-line word-diff highlights.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Utf16Span {
    /// Inclusive UTF-16 start offset within the line content.
    pub start: u32,
    /// Exclusive UTF-16 end offset within the line content.
    pub end: u32,
}

/// Bitmask constants for syntax token font styling (`LegionReviewDiffView` compatible).
pub struct FontStyleFlags;

impl FontStyleFlags {
    /// Normal upright regular weight text.
    pub const NONE: u8 = 0;
    /// Italic slant (`1 << 0`).
    pub const ITALIC: u8 = 1;
    /// Bold weight (`1 << 1`).
    pub const BOLD: u8 = 2;
    /// Underline decoration (`1 << 2`).
    pub const UNDERLINE: u8 = 4;
}

/// Input specification for appending one syntax token to a line inside [`DiffLineArrays`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffTokenInput {
    /// UTF-8 byte offset relative to the start of the line text.
    pub byte_offset: u32,
    /// UTF-8 byte length of the token slice.
    pub byte_len: u32,
    /// Optional packed `0xRRGGBB` / `0xAARRGGBB` foreground color.
    pub color: Option<u32>,
    /// Bitmask of [`FontStyleFlags`].
    pub font_style: u8,
}

/// Borrowed view over one syntax token inside [`DiffLineArrays`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffTokenView<'a> {
    /// Borrowed slice of the line text from [`DiffLineArrays`].
    pub text: &'a str,
    /// UTF-8 byte offset within the line.
    pub byte_offset: usize,
    /// UTF-8 byte length of the token.
    pub byte_len: usize,
    /// Optional packed color value.
    pub color: Option<u32>,
    /// Bitmask of [`FontStyleFlags`].
    pub font_style: u8,
}

/// Borrowed view over a single line stored in [`DiffLineArrays`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffLineView<'a> {
    /// Context, addition, deletion, or hunk header.
    pub kind: DiffLineKind,
    /// 0-based index of the owning hunk (`0` when outside hunks).
    pub hunk_index: u32,
    /// 1-based line number in the old file, if applicable.
    pub old_line_number: Option<u32>,
    /// 1-based line number in the new file, if applicable.
    pub new_line_number: Option<u32>,
    /// Line text borrowed directly from the contiguous `SoA` arena.
    pub text: &'a str,
    /// Word-diff UTF-16 highlight spans attached to this line.
    pub word_diffs: &'a [Utf16Span],
}

/// Structure-of-Arrays (`SoA`) line, word-diff, and syntax-token storage for parsed diffs.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffLineArrays {
    kinds: Vec<DiffLineKind>,
    hunk_indices: Vec<u32>,
    old_line_numbers: Vec<Option<u32>>,
    new_line_numbers: Vec<Option<u32>>,
    text_arena: String,
    text_ranges: Vec<TextRange>,
    word_diff_ranges: Vec<Utf16Span>,
    line_word_diff_spans: Vec<SliceRange>,
    token_spans: Vec<SliceRange>,
    token_byte_offsets: Vec<u32>,
    token_byte_lens: Vec<u32>,
    token_colors: Vec<Option<u32>>,
    token_font_styles: Vec<u8>,
}

impl DiffLineArrays {
    /// Creates an empty [`DiffLineArrays`] buffer.
    pub const fn new() -> Self {
        Self {
            kinds: Vec::new(),
            hunk_indices: Vec::new(),
            old_line_numbers: Vec::new(),
            new_line_numbers: Vec::new(),
            text_arena: String::new(),
            text_ranges: Vec::new(),
            word_diff_ranges: Vec::new(),
            line_word_diff_spans: Vec::new(),
            token_spans: Vec::new(),
            token_byte_offsets: Vec::new(),
            token_byte_lens: Vec::new(),
            token_colors: Vec::new(),
            token_font_styles: Vec::new(),
        }
    }

    /// Number of lines currently stored.
    pub fn len(&self) -> usize {
        self.kinds.len()
    }

    /// Returns `true` when zero lines are stored.
    pub fn is_empty(&self) -> bool {
        self.kinds.is_empty()
    }

    /// Total UTF-8 bytes stored across all line texts in the contiguous arena.
    pub fn text_arena_bytes(&self) -> usize {
        self.text_arena.len()
    }

    /// Total number of syntax tokens stored across all lines.
    pub fn total_tokens(&self) -> usize {
        self.token_byte_offsets.len()
    }

    /// Parallel slice of line kinds (`len() == self.len()`).
    pub fn kinds(&self) -> &[DiffLineKind] {
        &self.kinds
    }

    /// Parallel slice of 1-based old line numbers (`len() == self.len()`).
    pub fn old_line_numbers(&self) -> &[Option<u32>] {
        &self.old_line_numbers
    }

    /// Parallel slice of 1-based new line numbers (`len() == self.len()`).
    pub fn new_line_numbers(&self) -> &[Option<u32>] {
        &self.new_line_numbers
    }

    /// Appends one line to the `SoA` buffers, returning its 0-based line index.
    pub fn push_line(
        &mut self,
        kind: DiffLineKind,
        hunk_index: u32,
        old_line_number: Option<u32>,
        new_line_number: Option<u32>,
        text: &str,
    ) -> usize {
        let idx = self.kinds.len();
        let start = u32::try_from(self.text_arena.len()).unwrap_or(u32::MAX);
        let len = u32::try_from(text.len()).unwrap_or(u32::MAX);
        self.text_arena.push_str(text);

        self.kinds.push(kind);
        self.hunk_indices.push(hunk_index);
        self.old_line_numbers.push(old_line_number);
        self.new_line_numbers.push(new_line_number);
        self.text_ranges.push(TextRange { start, len });
        self.line_word_diff_spans.push(SliceRange {
            offset: u32::try_from(self.word_diff_ranges.len()).unwrap_or(u32::MAX),
            len: 0,
        });
        self.token_spans.push(SliceRange {
            offset: u32::try_from(self.token_byte_offsets.len()).unwrap_or(u32::MAX),
            len: 0,
        });
        idx
    }

    /// Attaches word-diff UTF-16 highlight spans to the line at `line_index`.
    ///
    /// Returns `false` if `line_index` is out of bounds.
    pub fn set_line_word_diffs(&mut self, line_index: usize, spans: &[Utf16Span]) -> bool {
        let Some(target) = self.line_word_diff_spans.get_mut(line_index) else {
            return false;
        };
        let offset = u32::try_from(self.word_diff_ranges.len()).unwrap_or(u32::MAX);
        let len = u32::try_from(spans.len()).unwrap_or(u32::MAX);
        self.word_diff_ranges.extend_from_slice(spans);
        *target = SliceRange { offset, len };
        true
    }

    /// Attaches contiguous syntax tokens to `line_index`, clamping invalid slices cleanly.
    pub fn set_line_tokens(&mut self, line_index: usize, tokens: &[DiffTokenInput]) -> bool {
        let Some(target) = self.token_spans.get_mut(line_index) else {
            return false;
        };
        let offset = u32::try_from(self.token_byte_offsets.len()).unwrap_or(u32::MAX);
        let len = u32::try_from(tokens.len()).unwrap_or(u32::MAX);
        for token in tokens {
            self.token_byte_offsets.push(token.byte_offset);
            self.token_byte_lens.push(token.byte_len);
            self.token_colors.push(token.color);
            self.token_font_styles.push(token.font_style);
        }
        *target = SliceRange { offset, len };
        true
    }

    /// Returns the borrowed text slice for `line_index`, or `None` if out of bounds.
    pub fn line_text(&self, line_index: usize) -> Option<&str> {
        let range = self.text_ranges.get(line_index)?;
        let start = usize::try_from(range.start).ok()?;
        let len = usize::try_from(range.len).ok()?;
        let end = start.checked_add(len)?;
        self.text_arena.get(start..end)
    }

    /// Returns a borrowed [`DiffLineView`] for `line_index`.
    pub fn line(&self, line_index: usize) -> Option<DiffLineView<'_>> {
        let kind = *self.kinds.get(line_index)?;
        let text = self.line_text(line_index)?;
        let word_diffs = self.line_word_diffs(line_index);
        Some(DiffLineView {
            kind,
            hunk_index: *self.hunk_indices.get(line_index)?,
            old_line_number: *self.old_line_numbers.get(line_index)?,
            new_line_number: *self.new_line_numbers.get(line_index)?,
            text,
            word_diffs,
        })
    }

    /// Returns the UTF-16 word-diff spans attached to `line_index`.
    pub fn line_word_diffs(&self, line_index: usize) -> &[Utf16Span] {
        let Some(span) = self.line_word_diff_spans.get(line_index) else {
            return &[];
        };
        let Ok(offset) = usize::try_from(span.offset) else {
            return &[];
        };
        let Ok(len) = usize::try_from(span.len) else {
            return &[];
        };
        let Some(end) = offset.checked_add(len) else {
            return &[];
        };
        self.word_diff_ranges.get(offset..end).unwrap_or(&[])
    }

    /// Iterates syntax tokens for `line_index` without heap allocation.
    pub fn line_tokens(&self, line_index: usize) -> Vec<DiffTokenView<'_>> {
        let Some(line_text) = self.line_text(line_index) else {
            return Vec::new();
        };
        let Some(span) = self.token_spans.get(line_index) else {
            return Vec::new();
        };
        let (Ok(offset), Ok(len)) = (usize::try_from(span.offset), usize::try_from(span.len))
        else {
            return Vec::new();
        };
        let Some(end) = offset.checked_add(len) else {
            return Vec::new();
        };
        if end > self.token_byte_offsets.len() {
            return Vec::new();
        }
        (offset..end)
            .filter_map(|idx| {
                let byte_offset = usize::try_from(self.token_byte_offsets[idx]).ok()?;
                let byte_len = usize::try_from(self.token_byte_lens[idx]).ok()?;
                let byte_end = byte_offset.checked_add(byte_len)?;
                let text = line_text.get(byte_offset..byte_end)?;
                Some(DiffTokenView {
                    text,
                    byte_offset,
                    byte_len,
                    color: self.token_colors[idx],
                    font_style: self.token_font_styles[idx],
                })
            })
            .collect()
    }
}
