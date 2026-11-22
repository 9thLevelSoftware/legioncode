//! `SoA`-optimized fuzzy command-palette search index and query helpers.
//!
//! Mirrors `packages/shared/src/paletteSearch.ts` using Structure-of-Arrays (`SoA`) storage:
//! precomputed 1:1 folded UTF-16 code units, packed `u64` word-boundary bitmasks, and batch
//! query execution returning ranked item indices and UTF-16 highlight spans without per-keystroke
//! heap allocations per item.

use crate::search_build::encode_field_into_arenas;
use crate::search_eval::{ItemFieldSpec, score_item_fields, sort_matches};
use crate::search_match::{FieldView, PreparedToken, is_filler_word, prepare_token};
pub use crate::search_query::{
    ASSISTANT_CITATION_LABEL_MAX_UTF16, MAX_CITATION_HREF_LENGTH, assistant_citation_label,
    extract_assistant_citation_hrefs, fold_search_query, normalize_whitespace,
    strip_thread_query_prefixes, strip_worktree_branch_prefix,
};

/// Half-open UTF-16 code-unit highlight span `[start, end)` within a field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SearchRange {
    /// Inclusive UTF-16 start offset.
    pub start: usize,
    /// Exclusive UTF-16 end offset.
    pub end: usize,
}

/// Word-boundary matching strictness for an indexed field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FieldMatchMode {
    /// Matches at word starts (and allows one-edit typo fallback when enabled).
    #[default]
    WordStart,
    /// Also allows 4+ unit substrings inside words (standard for primary thread titles).
    MidWord,
    /// Requires whole-word matches without typos.
    WholeWords,
    /// Hidden keyword field: requires word-start matches (`score >= 0.7`) without typos.
    KeywordWordStart,
    /// Hidden keyword field requiring exact whole-word matches without typos.
    KeywordWholeWords,
}

/// Matching policy for a single indexed field.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FieldOptions {
    /// Weight multiplier applied to token match scores in this field.
    pub weight: f64,
    /// Word-boundary and typo strictness mode.
    pub mode: FieldMatchMode,
    /// Context field (project/environment) that does not mark a hit strong for titles.
    pub context: bool,
}

impl Default for FieldOptions {
    fn default() -> Self {
        Self {
            weight: 1.0,
            mode: FieldMatchMode::WordStart,
            context: false,
        }
    }
}

/// Borrowed field specification passed to [`FuzzyPaletteIndex::from_entries`].
#[derive(Debug, Clone, Copy)]
pub struct PaletteField<'a> {
    /// Source UTF-8 text for the field.
    pub text: &'a str,
    /// Matching rules and scoring weight.
    pub options: FieldOptions,
}

impl<'a> PaletteField<'a> {
    /// Creates a primary title field (`weight = 1.0`, [`FieldMatchMode::MidWord`]).
    pub fn primary(text: &'a str) -> Self {
        Self {
            text,
            options: FieldOptions {
                weight: 1.0,
                mode: FieldMatchMode::MidWord,
                context: false,
            },
        }
    }
}

/// Multi-field item entry passed to [`FuzzyPaletteIndex::from_entries`].
#[derive(Debug, Clone)]
pub struct PaletteEntry<'a> {
    /// Ordered fields for this item; field `0` supplies highlight spans.
    pub fields: Vec<PaletteField<'a>>,
    /// Multiplicative usage/recency boost (`>= 1.0`).
    pub usage_boost: f64,
}

/// Options controlling batch [`FuzzyPaletteIndex::search_with_options`] execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchOptions {
    /// Fall back to one-edit typo tolerance when no item has a strong word-start match.
    pub allow_typos: bool,
    /// Optional cap on the number of ranked results returned.
    pub limit: Option<usize>,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            allow_typos: true,
            limit: None,
        }
    }
}

/// Ranked match emitted by [`FuzzyPaletteIndex`].
#[derive(Debug, Clone, PartialEq)]
pub struct PaletteMatch {
    /// Zero-based index of the matched item in insertion order.
    pub item_index: usize,
    /// Final ranking score (`quality * usage_boost`).
    pub score: f64,
    /// Pre-boost match quality (`~0.3..=1.5`).
    pub quality: f64,
    /// `true` when every non-filler token had `>= 3` units and matched a whole word at full weight.
    pub whole_words: bool,
    /// `true` when every token matched at a word start without a typo.
    pub strong: bool,
    /// Sorted, merged UTF-16 highlight ranges within the primary field.
    pub ranges: Vec<SearchRange>,
}

/// Structure-of-Arrays (`SoA`) batch fuzzy palette search index.
///
/// Precomputes 1:1 folded UTF-16 code units and packed `u64` word-boundary / alphanumeric bitmasks
/// in contiguous flat arenas so every keystroke evaluates the whole batch without allocating
/// per-item buffers.
///
/// # Examples
///
/// ```
/// use legion_shared::search::{FuzzyPaletteIndex, SearchRange};
/// use legion_shared::utf16::utf16_slice_exact;
///
/// let titles = [
///     "Fix oauth redirect",
///     "Authentication cleanup",
///     "Share auth across worktrees",
/// ];
/// let index = FuzzyPaletteIndex::new(titles);
/// let hits = index.search("auth");
///
/// assert_eq!(hits.iter().map(|m| m.item_index).collect::<Vec<_>>(), vec![2, 1, 0]);
/// assert_eq!(hits[0].ranges, vec![SearchRange { start: 6, end: 10 }]);
/// assert_eq!(
///     utf16_slice_exact(titles[hits[0].item_index], hits[0].ranges[0].start, hits[0].ranges[0].end),
///     Ok("auth")
/// );
/// ```
#[derive(Debug, Clone, Default)]
pub struct FuzzyPaletteIndex {
    item_field_start: Vec<u32>,
    item_field_count: Vec<u16>,
    item_usage_boost: Vec<f64>,

    field_text_offset: Vec<u32>,
    field_text_len: Vec<u32>,
    field_mask_offset: Vec<u32>,
    field_weight: Vec<f64>,
    field_flags: Vec<u8>,

    folded_utf16: Vec<u16>,
    word_start_masks: Vec<u64>,
    alnum_masks: Vec<u64>,
}

impl FuzzyPaletteIndex {
    /// Builds a single-field [`FuzzyPaletteIndex`] from an iterable of item titles.
    pub fn new<I, S>(titles: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let iter = titles.into_iter();
        let (lower, _) = iter.size_hint();
        let mut index = Self {
            item_field_start: Vec::with_capacity(lower),
            item_field_count: Vec::with_capacity(lower),
            item_usage_boost: Vec::with_capacity(lower),
            ..Self::default()
        };
        for title in iter {
            index.push_entry(&PaletteEntry {
                fields: vec![PaletteField::primary(title.as_ref())],
                usage_boost: 1.0,
            });
        }
        index
    }

    /// Builds a multi-field weighted [`FuzzyPaletteIndex`] from structured [`PaletteEntry`] items.
    pub fn from_entries<'a, I>(entries: I) -> Self
    where
        I: IntoIterator<Item = PaletteEntry<'a>>,
    {
        let mut index = Self::default();
        for entry in entries {
            index.push_entry(&entry);
        }
        index
    }

    /// Appends one multi-field entry to the `SoA` arenas.
    pub fn push_entry(&mut self, entry: &PaletteEntry<'_>) {
        let field_start = u32::try_from(self.field_text_offset.len()).unwrap_or(u32::MAX);
        let field_count = u16::try_from(entry.fields.len()).unwrap_or(u16::MAX);
        self.item_field_start.push(field_start);
        self.item_field_count.push(field_count);
        self.item_usage_boost.push(
            if entry.usage_boost.is_finite() && entry.usage_boost > 0.0 {
                entry.usage_boost
            } else {
                1.0
            },
        );

        for (slot, field) in entry
            .fields
            .iter()
            .take(usize::from(field_count))
            .enumerate()
        {
            let meta = encode_field_into_arenas(
                slot == 0,
                field,
                &mut self.folded_utf16,
                &mut self.word_start_masks,
                &mut self.alnum_masks,
            );
            self.field_text_offset.push(meta.text_offset);
            self.field_text_len.push(meta.text_len);
            self.field_mask_offset.push(meta.mask_offset);
            self.field_weight.push(meta.weight);
            self.field_flags.push(meta.flags);
        }
    }

    /// Returns the number of indexed items.
    #[inline]
    pub fn len(&self) -> usize {
        self.item_field_start.len()
    }

    /// Returns `true` when the index holds zero items.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.item_field_start.is_empty()
    }

    /// Returns the precomputed folded UTF-16 slice for `item_index` and `field_slot`.
    pub fn folded_field_utf16(&self, item_index: usize, field_slot: usize) -> Option<&[u16]> {
        let field_idx = self.resolve_field_index(item_index, field_slot)?;
        let offset = self.field_text_offset[field_idx] as usize;
        let len = self.field_text_len[field_idx] as usize;
        Some(&self.folded_utf16[offset..offset + len])
    }

    /// Returns `true` if `utf16_offset` marks a word boundary in `(item_index, field_slot)`.
    pub fn is_word_start(&self, item_index: usize, field_slot: usize, utf16_offset: usize) -> bool {
        let Some(field_idx) = self.resolve_field_index(item_index, field_slot) else {
            return false;
        };
        let len = self.field_text_len[field_idx] as usize;
        if utf16_offset >= len {
            return false;
        }
        let mask_off = self.field_mask_offset[field_idx] as usize;
        let word = mask_off + (utf16_offset >> 6);
        ((self.word_start_masks[word] >> (utf16_offset & 63)) & 1) != 0
    }

    /// Ranks all indexed items matching `query` with default typo-fallback options.
    pub fn search(&self, query: &str) -> Vec<PaletteMatch> {
        self.search_with_options(query, SearchOptions::default())
    }

    /// Ranks indexed items matching `query` according to `options`.
    pub fn search_with_options(&self, query: &str, options: SearchOptions) -> Vec<PaletteMatch> {
        let normalized = fold_search_query(query);
        if normalized.is_empty() || self.is_empty() {
            return Vec::new();
        }
        let query_u16: Vec<u16> = normalized.encode_utf16().collect();
        let tokens: Vec<PreparedToken> = normalized.split(' ').map(prepare_token).collect();
        if tokens.is_empty() {
            return Vec::new();
        }
        let filler_optional = normalized.split(' ').any(|tok| !is_filler_word(tok));

        let mut matches = self.evaluate_batch(&query_u16, &tokens, filler_optional, false);
        let has_strong = matches.iter().any(|m| m.strong);
        if !has_strong && options.allow_typos {
            matches = self.evaluate_batch(&query_u16, &tokens, filler_optional, true);
        }

        sort_matches(&mut matches);
        if let Some(limit) = options.limit {
            matches.truncate(limit);
        }
        matches
    }

    fn evaluate_batch(
        &self,
        query_u16: &[u16],
        tokens: &[PreparedToken],
        filler_optional: bool,
        typos: bool,
    ) -> Vec<PaletteMatch> {
        let mut results = Vec::new();
        let mut scratch: Vec<ItemFieldSpec<'_>> = Vec::new();
        for item_idx in 0..self.len() {
            let f_start = self.item_field_start[item_idx] as usize;
            let f_end = f_start + usize::from(self.item_field_count[item_idx]);
            scratch.clear();
            for field_idx in f_start..f_end {
                scratch.push(ItemFieldSpec {
                    view: self.field_view(field_idx),
                    weight: self.field_weight[field_idx],
                    flags: self.field_flags[field_idx],
                });
            }
            if let Some(m) = score_item_fields(
                item_idx,
                self.item_usage_boost[item_idx],
                &scratch,
                query_u16,
                tokens,
                filler_optional,
                typos,
            ) {
                results.push(m);
            }
        }
        results
    }

    fn resolve_field_index(&self, item_index: usize, field_slot: usize) -> Option<usize> {
        let count = usize::from(*self.item_field_count.get(item_index)?);
        if field_slot >= count {
            return None;
        }
        Some(self.item_field_start[item_index] as usize + field_slot)
    }

    fn field_view(&self, field_idx: usize) -> FieldView<'_> {
        let text_off = self.field_text_offset[field_idx] as usize;
        let text_len = self.field_text_len[field_idx] as usize;
        let mask_off = self.field_mask_offset[field_idx] as usize;
        let mask_words = text_len.div_ceil(64);
        FieldView {
            units: &self.folded_utf16[text_off..text_off + text_len],
            word_starts: &self.word_start_masks[mask_off..mask_off + mask_words],
            alnum: &self.alnum_masks[mask_off..mask_off + mask_words],
        }
    }
}
