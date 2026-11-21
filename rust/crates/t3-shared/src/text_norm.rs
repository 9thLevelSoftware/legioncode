//! Canonical NFD->NFC composition, accent folding, safe UTF-8/UTF-16/grapheme truncation,
//! and deduplicated pending-subagent counting.

use std::borrow::Cow;
use std::collections::HashSet;

use crate::text_norm_tables::compose_pair;
use crate::utf16::{fold_char_in_place, splits_surrogate_pair, utf8_len_to_utf16};

const fn combining_class(ch: char) -> u8 {
    match ch as u32 {
        0x0300..=0x0314
        | 0x033D..=0x0344
        | 0x0346
        | 0x034A..=0x034C
        | 0x0350..=0x0352
        | 0x0357
        | 0x035B
        | 0x0363..=0x036F
        | 0x1AB0..=0x1AFF
        | 0x1DC0..=0x1DFF => 230,
        0x0315 | 0x031A | 0x0358 => 232,
        0x0316..=0x0319
        | 0x031C..=0x0320
        | 0x0323..=0x0326
        | 0x0329..=0x0333
        | 0x0339..=0x033C
        | 0x0347..=0x0349
        | 0x034D..=0x034E
        | 0x0353..=0x0356
        | 0x0359..=0x035A => 220,
        0x031B => 216,
        0x0321..=0x0322 | 0x0327..=0x0328 => 202,
        0x0334..=0x0338 => 1,
        0x0345 => 240,
        0x035C | 0x035F | 0x0362 => 233,
        0x035D..=0x035E | 0x0360..=0x0361 => 234,
        _ => 0,
    }
}

const fn is_combining_diacritic(ch: char) -> bool {
    matches!(
        ch as u32,
        0x0300..=0x036F
            | 0x1AB0..=0x1AFF
            | 0x1DC0..=0x1DFF
            | 0x20D0..=0x20FF
            | 0xFE20..=0xFE2F
    )
}

fn is_hangul_jamo_or_mark(ch: char) -> bool {
    let code = ch as u32;
    combining_class(ch) != 0 || (0x1161..=0x11C2).contains(&code)
}

/// Composes canonically decomposed sequences (including macOS NFD filenames and Hangul Jamo) to NFC.
#[must_use]
pub fn normalize_nfc(input: &str) -> Cow<'_, str> {
    if input.is_ascii() || !input.chars().any(is_hangul_jamo_or_mark) {
        return Cow::Borrowed(input);
    }

    let mut chars: Vec<char> = input.chars().collect();
    // Canonical ordering (UAX #15): stable-sort runs of non-zero combining class marks.
    let mut start = 0usize;
    while start < chars.len() {
        if combining_class(chars[start]) == 0 {
            start += 1;
            continue;
        }
        let mut end = start + 1;
        while end < chars.len() && combining_class(chars[end]) != 0 {
            end += 1;
        }
        chars[start..end].sort_by_key(|&c| combining_class(c));
        start = end;
    }

    let mut out: Vec<char> = Vec::with_capacity(chars.len());
    let mut starter_idx: Option<usize> = None;
    let mut prev_ccc: u8 = 0;

    for ch in chars {
        let ccc = combining_class(ch);
        if let Some(idx) = starter_idx {
            let blocked = ccc != 0 && prev_ccc >= ccc;
            if !blocked
                && (ccc != 0 || prev_ccc == 0)
                && let Some(composed) = compose_pair(out[idx], ch)
            {
                out[idx] = composed;
                continue;
            }
        }
        if ccc == 0 {
            starter_idx = Some(out.len());
            prev_ccc = 0;
        } else {
            prev_ccc = ccc;
        }
        out.push(ch);
    }

    Cow::Owned(out.into_iter().collect())
}

/// Allocates an NFC-composed `String` copy of `input`.
#[must_use]
pub fn normalize_nfc_string(input: &str) -> String {
    normalize_nfc(input).into_owned()
}

/// Extracts the final path component across `/` or `\` separators and composes NFD sequences to NFC (`e28b2819ee`).
#[must_use]
pub fn file_display_name(path: &str) -> String {
    let trimmed = path.trim_end_matches(['/', '\\']);
    let basename = if trimmed.is_empty() {
        path
    } else if let Some(idx) = trimmed.rfind(['/', '\\']) {
        &trimmed[idx + 1..]
    } else {
        trimmed
    };
    normalize_nfc(basename).into_owned()
}

/// Folds Latin/Greek/Cyrillic accents and case so accent-insensitive search matches `"café"` via `"cafe"` (`732c03f410`).
#[must_use]
pub fn fold_accents(input: &str) -> String {
    if input.is_ascii() {
        return input.to_ascii_lowercase();
    }
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        if is_combining_diacritic(ch) {
            continue;
        }
        out.push(fold_char_in_place(ch));
    }
    out
}

/// Returns `true` when `haystack` contains `needle` after accent & case folding.
#[must_use]
pub fn accent_folded_contains(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    fold_accents(haystack).contains(&fold_accents(needle))
}

/// Returns `true` if slicing `text` at `utf16_offset` code units splits a surrogate pair (`272cd38379`).
#[must_use]
pub fn splits_surrogate_pair_at(text: &str, utf16_offset: usize) -> bool {
    let units: Vec<u16> = text.encode_utf16().collect();
    splits_surrogate_pair(&units, utf16_offset)
}

/// Truncates a prefix of `text` to at most `max_chars` Unicode scalar values without panicking.
#[must_use]
pub fn truncate_prefix_chars(text: &str, max_chars: usize) -> &str {
    if max_chars == 0 {
        return "";
    }
    match text.char_indices().nth(max_chars) {
        Some((byte_idx, _)) => &text[..byte_idx],
        None => text,
    }
}

/// Truncates a prefix of `text` to at most `max_utf16` UTF-16 code units without splitting any surrogate pair (`272cd38379`, `c688453acd`).
#[must_use]
pub fn truncate_prefix_utf16(text: &str, max_utf16: usize) -> &str {
    if utf8_len_to_utf16(text) <= max_utf16 {
        return text;
    }
    let mut used = 0usize;
    let mut cut_byte = 0usize;
    for (byte_idx, ch) in text.char_indices() {
        let len = ch.len_utf16();
        if used + len > max_utf16 {
            cut_byte = byte_idx;
            break;
        }
        used += len;
        cut_byte = byte_idx + ch.len_utf8();
    }
    &text[..cut_byte]
}

/// Retains a suffix (tail) of `text` spanning at most `max_utf16` UTF-16 code units without splitting a surrogate pair (`df09ced768`).
#[must_use]
pub fn truncate_tail_utf16(text: &str, max_utf16: usize) -> &str {
    let total = utf8_len_to_utf16(text);
    if total <= max_utf16 {
        return text;
    }
    let to_drop = total - max_utf16;
    let mut dropped = 0usize;
    for (byte_idx, ch) in text.char_indices() {
        if dropped >= to_drop {
            return &text[byte_idx..];
        }
        dropped += ch.len_utf16();
    }
    ""
}

const fn is_grapheme_extend(ch: char) -> bool {
    is_combining_diacritic(ch)
        || matches!(
            ch as u32,
            0x200C
                | 0x200D
                | 0xFE00..=0xFE0F
                | 0xE0100..=0xE01EF
                | 0x1F3FB..=0x1F3FF
                | 0xE0020..=0xE007F
                | 0x1160..=0x11FF
        )
}

const fn is_regional_indicator(ch: char) -> bool {
    matches!(ch as u32, 0x1F1E6..=0x1F1FF)
}

/// Truncates a prefix of `text` to at most `max_clusters` extended grapheme clusters so combining sequences, flags, and ZWJ emoji remain intact.
#[must_use]
pub fn truncate_prefix_graphemes(text: &str, max_clusters: usize) -> &str {
    if max_clusters == 0 {
        return "";
    }
    let mut count = 0usize;
    let mut prev: Option<char> = None;
    let mut ri_run = 0usize;

    for (byte_idx, ch) in text.char_indices() {
        let joined = matches!(prev, None | Some('\u{200D}'))
            || matches!(prev, Some('\r') if ch == '\n')
            || (prev.is_some()
                && (is_grapheme_extend(ch) || (is_regional_indicator(ch) && ri_run % 2 == 1)));

        if prev.is_some() && !joined {
            count += 1;
            if count >= max_clusters {
                return &text[..byte_idx];
            }
        }

        if prev.is_some() && !joined {
            ri_run = usize::from(is_regional_indicator(ch));
        } else if is_regional_indicator(ch) {
            ri_run += 1;
        } else {
            ri_run = 0;
        }
        prev = Some(ch);
    }

    text
}

/// Filters pending background-work items so duplicate subagent tasks referencing the same `child_thread_id` are counted once (`9bb92a46dd`).
#[must_use]
pub fn dedup_pending_subagent_threads<T, F>(items: Vec<T>, mut subagent_child_id: F) -> Vec<T>
where
    F: FnMut(&T) -> Option<String>,
{
    let mut seen_children = HashSet::new();
    items
        .into_iter()
        .filter(|item| match subagent_child_id(item) {
            None => true,
            Some(child_id) => seen_children.insert(child_id),
        })
        .collect()
}

/// Counts unique pending subagent `child_thread_id` values (`9bb92a46dd`), treating `None` entries as distinct standalone tasks.
#[must_use]
pub fn count_unique_pending_subagent_threads<'a, I>(child_thread_ids: I) -> usize
where
    I: IntoIterator<Item = Option<&'a str>>,
{
    let mut seen = HashSet::new();
    let mut count = 0usize;
    for id in child_thread_ids {
        match id {
            None => count += 1,
            Some(thread_id) if seen.insert(thread_id) => count += 1,
            Some(_) => {}
        }
    }
    count
}
