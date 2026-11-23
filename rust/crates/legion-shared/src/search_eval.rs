//! Batch item scoring and deterministic ranking over [`crate::search::FuzzyPaletteIndex`] `SoA` arenas.

use std::cmp::Ordering;

use crate::search::{PaletteMatch, SearchRange};
use crate::search_build::{
    FLAG_CONTEXT, FLAG_KEYWORD, FLAG_MID_WORD, FLAG_PRIMARY, FLAG_WHOLE_WORDS,
};
use crate::search_match::{
    FieldView, PHRASE_BONUS, PICK_PREFIX_MIN_LENGTH, PreparedToken, WHOLE_NAME_BONUS,
    WHOLE_WORD_SCORE, contains_subslice, match_class, match_token, merge_ranges,
};

pub(crate) struct ItemFieldSpec<'a> {
    pub(crate) view: FieldView<'a>,
    pub(crate) weight: f64,
    pub(crate) flags: u8,
}

pub(crate) fn score_item_fields(
    item_idx: usize,
    usage_boost: f64,
    field_views: &[ItemFieldSpec<'_>],
    query_u16: &[u16],
    tokens: &[PreparedToken],
    filler_optional: bool,
    typos: bool,
) -> Option<PaletteMatch> {
    let primary_non_empty = field_views
        .first()
        .is_some_and(|first| !first.view.units.is_empty());

    let mut primary_ranges: Vec<SearchRange> = Vec::new();
    let mut total = 0.0f64;
    let mut counted = 0usize;
    let mut not_whole = false;
    let mut strong = true;

    for token in tokens {
        let mut best = 0.0f64;
        let mut best_primary = None;
        let mut at_word_start = false;

        for spec in field_views {
            if spec.view.units.is_empty() {
                continue;
            }
            let is_primary = (spec.flags & FLAG_PRIMARY) != 0;
            if best >= spec.weight && !is_primary && at_word_start {
                continue;
            }
            let keyword = (spec.flags & FLAG_KEYWORD) != 0;
            let whole_words = (spec.flags & FLAG_WHOLE_WORDS) != 0;
            let context = (spec.flags & FLAG_CONTEXT) != 0;
            let mid_word = (spec.flags & FLAG_MID_WORD) != 0;

            let Some(found) =
                match_token(spec.view, token, typos && !keyword, is_primary, mid_word)
            else {
                continue;
            };
            let min_score = if whole_words {
                WHOLE_WORD_SCORE
            } else if keyword {
                0.7
            } else {
                0.0
            };
            if found.score < min_score {
                continue;
            }
            if found.score >= 0.7 && !context {
                at_word_start = true;
            }
            if found.score * spec.weight > best {
                best = found.score * spec.weight;
            }
            if is_primary {
                best_primary = Some(found);
            }
        }

        if best <= 0.0 {
            if filler_optional && token.is_filler {
                continue;
            }
            return None;
        }
        strong &= at_word_start;
        if let Some(primary_hit) = best_primary {
            if let Some(letters) = primary_hit.letters {
                primary_ranges.extend(letters);
            } else {
                primary_ranges.push(SearchRange {
                    start: primary_hit.start,
                    end: primary_hit.end,
                });
            }
        }
        total += best;
        counted += 1;
        if token.is_filler {
            not_whole |= !filler_optional;
        } else {
            not_whole |= token.u16_units.len() < PICK_PREFIX_MIN_LENGTH || best < WHOLE_WORD_SCORE;
        }
    }

    if counted == 0 {
        return None;
    }
    let mut quality = total / f64::from(u32::try_from(counted).unwrap_or(u32::MAX));
    if primary_non_empty && let Some(first) = field_views.first() {
        let primary_units = first.view.units;
        if primary_units == query_u16 {
            quality += WHOLE_NAME_BONUS;
        } else if tokens.len() > 1 && contains_subslice(primary_units, query_u16) {
            quality += PHRASE_BONUS;
        }
    }

    Some(PaletteMatch {
        item_index: item_idx,
        score: quality * usage_boost,
        quality,
        whole_words: !not_whole,
        strong,
        ranges: merge_ranges(&mut primary_ranges),
    })
}

pub(crate) fn sort_matches(matches: &mut [PaletteMatch]) {
    matches.sort_by(|left, right| {
        let left_word_match = match_class(left.quality).min(2);
        let right_word_match = match_class(right.quality).min(2);
        right_word_match
            .cmp(&left_word_match)
            .then_with(|| match_class(right.quality).cmp(&match_class(left.quality)))
            .then_with(|| right.whole_words.cmp(&left.whole_words))
            .then_with(|| {
                right
                    .score
                    .partial_cmp(&left.score)
                    .unwrap_or(Ordering::Equal)
            })
            .then_with(|| left.item_index.cmp(&right.item_index))
    });
}
