//! Single-edit typo matching for [`crate::search`].

use crate::search_match::{FieldView, TYPO_SCORE, TokenMatch, find_set_bit_from};
use crate::utf16::is_low_surrogate;

pub(crate) fn match_typo_in_word_starts(field: FieldView<'_>, token: &[u16]) -> Option<TokenMatch> {
    let mut pos = 0usize;
    while let Some(index) = find_set_bit_from(field.word_starts, field.len(), pos) {
        pos = index + 1;
        let first = field.units[index];
        let first_kept = first == token[0]
            || (token.len() > 3
                && first == token[1]
                && index + 1 < field.len()
                && field.units[index + 1] == token[0]);
        if !first_kept {
            continue;
        }

        let run_end = field.run_end(index + 1);
        let word = &field.units[index..run_end];
        let short = token.len() <= 4;
        let candidates: &[usize] = if short {
            &[token.len(), token.len() + 1]
        } else {
            &[token.len(), token.len() - 1, token.len() + 1]
        };
        for &length in candidates {
            if length > word.len() {
                continue;
            }
            if index + length < field.len() && is_low_surrogate(field.units[index + length]) {
                continue;
            }
            if within_one_edit(token, &word[..length], short && length == token.len()) {
                return Some(TokenMatch {
                    score: TYPO_SCORE,
                    start: index,
                    end: index + length,
                    letters: None,
                });
            }
        }
    }
    None
}

fn within_one_edit(left: &[u16], right: &[u16], swap_only: bool) -> bool {
    let max_diff = usize::from(!swap_only);
    if left.len().abs_diff(right.len()) > max_diff {
        return false;
    }
    let mut index = 0usize;
    while index < left.len() && index < right.len() && left[index] == right[index] {
        index += 1;
    }
    let rest_left = &left[index..];
    let rest_right = &right[index..];
    let swapped = rest_left.len() > 1
        && rest_right.len() > 1
        && rest_left[0] == rest_right[1]
        && rest_left[1] == rest_right[0]
        && rest_left[2..] == rest_right[2..];
    if swap_only {
        return swapped;
    }
    swapped
        || rest_left == rest_right
        || (!rest_left.is_empty() && !rest_right.is_empty() && rest_left[1..] == rest_right[1..])
        || (!rest_left.is_empty() && rest_left[1..] == *rest_right)
        || (!rest_right.is_empty() && *rest_left == rest_right[1..])
}
