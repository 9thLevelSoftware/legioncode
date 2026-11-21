//! Token matching, typo tolerance, span merging, and deterministic ranking for [`crate::search`].

use crate::search::SearchRange;
use crate::search_typo::match_typo_in_word_starts;
use crate::unicode_so_table::is_symbol_other;

pub(crate) const WHOLE_WORD_SCORE: f64 = 1.0;
pub(crate) const WHOLE_NAME_BONUS: f64 = 0.5;
pub(crate) const PHRASE_BONUS: f64 = 0.25;
pub(crate) const MID_WORD_SCORE: f64 = 0.3;
pub(crate) const TYPO_SCORE: f64 = 0.45;
pub(crate) const JOINED_WORDS_SCORE: f64 = 0.6;
pub(crate) const PICK_PREFIX_MIN_LENGTH: usize = 3;

const FILLER_WORDS: &[&str] = &[
    "a", "an", "and", "for", "in", "of", "on", "the", "to", "with",
];

#[derive(Debug, Clone)]
pub(crate) struct PreparedToken {
    pub(crate) u16_units: Vec<u16>,
    pub(crate) own_boundary: bool,
    pub(crate) has_letter: bool,
    pub(crate) is_filler: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct TokenMatch {
    pub(crate) score: f64,
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) letters: Option<Vec<SearchRange>>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct FieldView<'a> {
    pub(crate) units: &'a [u16],
    pub(crate) word_starts: &'a [u64],
    pub(crate) alnum: &'a [u64],
}

impl FieldView<'_> {
    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.units.len()
    }

    #[inline]
    pub(crate) fn bit(words: &[u64], idx: usize) -> bool {
        let word = idx >> 6;
        word < words.len() && ((words[word] >> (idx & 63)) & 1) != 0
    }

    #[inline]
    pub(crate) fn is_word_start(&self, idx: usize) -> bool {
        idx < self.len() && Self::bit(self.word_starts, idx)
    }

    #[inline]
    pub(crate) fn is_alnum(&self, idx: usize) -> bool {
        idx < self.len() && Self::bit(self.alnum, idx)
    }

    pub(crate) fn word_end(&self, index: usize) -> usize {
        let mut end = index + 1;
        while end < self.len() && !self.is_word_start(end) && self.is_alnum(end) {
            end += 1;
        }
        end
    }

    pub(crate) fn run_end(&self, start: usize) -> usize {
        let mut end = start;
        while end < self.len() && self.is_alnum(end) {
            end += 1;
        }
        end
    }
}

pub(crate) fn is_filler_word(token: &str) -> bool {
    FILLER_WORDS.contains(&token)
}

pub(crate) fn prepare_token(token: &str) -> PreparedToken {
    let u16_units: Vec<u16> = token.encode_utf16().collect();
    let first_ch = token.chars().next();
    let own_boundary = first_ch.is_some_and(|ch| !ch.is_alphanumeric() && !is_symbol_other(ch));
    let has_letter = token.chars().any(char::is_alphabetic);
    PreparedToken {
        u16_units,
        own_boundary,
        has_letter,
        is_filler: is_filler_word(token),
    }
}

pub(crate) fn contains_subslice(haystack: &[u16], needle: &[u16]) -> bool {
    find_subslice_from(haystack, needle, 0).is_some()
}

pub(crate) fn find_subslice_from(haystack: &[u16], needle: &[u16], from: usize) -> Option<usize> {
    if needle.is_empty() || from + needle.len() > haystack.len() {
        return None;
    }
    let first = needle[0];
    let limit = haystack.len() - needle.len();
    let mut i = from;
    while i <= limit {
        if haystack[i] == first && haystack[i..i + needle.len()] == *needle {
            return Some(i);
        }
        i += 1;
    }
    None
}

pub(crate) fn match_token(
    field: FieldView<'_>,
    token: &PreparedToken,
    typos: bool,
    is_primary: bool,
    mid_word: bool,
) -> Option<TokenMatch> {
    let needle = &token.u16_units;
    if needle.is_empty() || needle.len() > field.len() {
        return None;
    }

    let mut best: Option<TokenMatch> = None;
    let mut cursor = 0usize;
    while let Some(index) = find_subslice_from(field.units, needle, cursor) {
        if field.is_word_start(index) || token.own_boundary {
            let end = index + needle.len();
            let whole = end >= field.len() || field.is_word_start(end) || !field.is_alnum(end);
            let run_end = field.run_end(end);
            let run_len = f64::from(u32::try_from(run_end - index).unwrap_or(u32::MAX));
            let needle_len = f64::from(u32::try_from(needle.len()).unwrap_or(u32::MAX));
            let score = if whole {
                WHOLE_WORD_SCORE
            } else {
                0.7 + (0.2 * needle_len) / run_len
            };
            if best.as_ref().is_none_or(|b| score > b.score) {
                best = Some(TokenMatch {
                    score,
                    start: index,
                    end,
                    letters: None,
                });
            }
            if (score - WHOLE_WORD_SCORE).abs() < f64::EPSILON {
                return best;
            }
        } else if mid_word && needle.len() >= 4 && best.is_none() {
            best = Some(TokenMatch {
                score: MID_WORD_SCORE,
                start: index,
                end: index + needle.len(),
                letters: None,
            });
        }
        cursor = index + 1;
    }

    if best.as_ref().is_none_or(|b| b.score <= MID_WORD_SCORE)
        && is_primary
        && let Some(joined) = match_joined_words(field, needle)
    {
        return Some(joined);
    }

    if best.as_ref().is_some_and(|b| b.score > MID_WORD_SCORE)
        || !typos
        || needle.len() < 3
        || !token.has_letter
    {
        return best;
    }

    match_typo_in_word_starts(field, needle).or(best)
}

fn match_joined_words(field: FieldView<'_>, token: &[u16]) -> Option<TokenMatch> {
    let first = *token.first()?;
    let first_word_start = first_set_bit(field.word_starts, field.len())?;

    let mut pos = 0usize;
    while let Some(start) = find_set_bit_from(field.word_starts, field.len(), pos) {
        if field.units[start] == first
            && let Some(matched) = try_joined_from(field, token, start, first_word_start)
        {
            return Some(matched);
        }
        pos = start + 1;
    }
    None
}

fn try_joined_from(
    field: FieldView<'_>,
    token: &[u16],
    start: usize,
    first_word_start: usize,
) -> Option<TokenMatch> {
    let mut runs: Vec<SearchRange> = Vec::new();
    let mut at = start;
    let mut rest = token;

    while !rest.is_empty() && at < field.len() {
        let end = field.word_end(at);
        let seg_len = end - at;
        if rest.len() <= seg_len && field.units[at..at + rest.len()] == *rest {
            runs.push(SearchRange {
                start: at,
                end: at + rest.len(),
            });
            rest = &[];
        } else if rest.len() >= seg_len && rest[..seg_len] == field.units[at..end] {
            runs.push(SearchRange { start: at, end });
            rest = &rest[seg_len..];
            at = end;
            while at < field.len() && !field.is_word_start(at) {
                at += 1;
            }
        } else {
            break;
        }
    }

    let last = runs.last().copied()?;
    if !rest.is_empty() || runs.len() < 2 {
        return None;
    }

    let all_prefix_ge_3 = runs[..runs.len() - 1].iter().all(|r| r.end - r.start >= 3);
    if last.end == field.word_end(last.start)
        || last.end - last.start >= 3
        || start == first_word_start
        || all_prefix_ge_3
    {
        Some(TokenMatch {
            score: JOINED_WORDS_SCORE,
            start,
            end: last.end,
            letters: Some(runs),
        })
    } else {
        None
    }
}

pub(crate) fn merge_ranges(ranges: &mut [SearchRange]) -> Vec<SearchRange> {
    if ranges.is_empty() {
        return Vec::new();
    }
    ranges.sort_unstable_by_key(|r| (r.start, r.end));
    let mut merged: Vec<SearchRange> = Vec::with_capacity(ranges.len());
    for range in ranges.iter().copied() {
        if let Some(last) = merged.last_mut()
            && range.start <= last.end
        {
            last.end = last.end.max(range.end);
        } else {
            merged.push(range);
        }
    }
    merged
}

#[inline]
pub(crate) fn match_class(quality: f64) -> u8 {
    if quality >= 1.5 {
        3
    } else if quality >= 0.7 {
        2
    } else {
        u8::from(quality >= 0.4)
    }
}

fn first_set_bit(words: &[u64], len: usize) -> Option<usize> {
    find_set_bit_from(words, len, 0)
}

pub(crate) fn find_set_bit_from(words: &[u64], len: usize, start: usize) -> Option<usize> {
    if start >= len {
        return None;
    }
    let mut word_idx = start >> 6;
    let bit_offset = start & 63;
    if word_idx < words.len() {
        let masked = words[word_idx] & (!0u64 << bit_offset);
        if masked != 0 {
            let bit = (word_idx << 6) + masked.trailing_zeros() as usize;
            return (bit < len).then_some(bit);
        }
        word_idx += 1;
    }
    while word_idx < words.len() {
        let word = words[word_idx];
        if word != 0 {
            let bit = (word_idx << 6) + word.trailing_zeros() as usize;
            return (bit < len).then_some(bit);
        }
        word_idx += 1;
    }
    None
}
