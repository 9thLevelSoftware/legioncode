//! Fuzzy matching, Soundex, edit-distance, and token boundary helpers for
//! [`crate::custom_words::SpeechTextCorrector`].

use std::collections::HashSet;

use crate::custom_words::MAX_SPEECH_WORD_LEN;

const FUZZY_MATCH_THRESHOLD: f64 = 0.18;

#[derive(Debug, Clone)]
pub(crate) struct CustomWordKey {
    pub(crate) word: String,
    pub(crate) key: String,
    pub(crate) inner_possessives: usize,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct WordToken {
    pub(crate) start: usize,
    pub(crate) end: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct FuzzyMatch {
    pub(crate) word: String,
    pub(crate) score: f64,
    pub(crate) suffix: String,
}

pub(crate) fn build_custom_word_keys(words: &[String]) -> Vec<CustomWordKey> {
    let mut keys = Vec::new();
    for word in words {
        let mut sources = vec![word.clone()];
        if word.contains('&') {
            sources.push(word.replace('&', " and "));
        }
        let mut seen_local = HashSet::new();
        for source in sources {
            let key = match_key(&source);
            let compact: String = source
                .chars()
                .filter(|ch| !ch.is_whitespace())
                .collect::<String>()
                .to_lowercase();
            let lost_symbols = key != compact;
            if (key.chars().count() < 2 && lost_symbols)
                || !is_ascii_alnum(&key)
                || !seen_local.insert(key.clone())
            {
                continue;
            }
            keys.push(CustomWordKey {
                word: word.clone(),
                key,
                inner_possessives: count_inner_possessives(word),
            });
        }
    }
    keys
}

pub(crate) fn best_fuzzy_match(phrase: &str, keys: &[CustomWordKey]) -> Option<FuzzyMatch> {
    let key = match_key(phrase);
    let (stem, suffix) = split_possessive(phrase, &key);
    if !is_ascii_alnum(stem) || stem.len() > MAX_SPEECH_WORD_LEN {
        return None;
    }
    let inner = count_inner_possessives(phrase);
    let mut best: Option<FuzzyMatch> = None;
    for custom in keys {
        if inner > 0 && inner > custom.inner_possessives {
            continue;
        }
        let keep_suffix = !suffix.is_empty() && !ends_with_possessive(&custom.word);
        let candidate = if keep_suffix { stem } else { key.as_str() };
        let min_len = candidate.len().min(custom.key.len());
        if min_len < 4 && candidate != custom.key {
            continue;
        }
        let longest = candidate.len().max(custom.key.len());
        let len_diff = candidate.len().abs_diff(custom.key.len());
        let longest_f = u32::try_from(longest).unwrap_or(u32::MAX);
        if f64::from(u32::try_from(len_diff).unwrap_or(u32::MAX))
            > (f64::from(longest_f) * 0.25).max(2.0)
        {
            continue;
        }
        let dist = edit_distance(candidate.as_bytes(), custom.key.as_bytes());
        let raw_score = f64::from(u32::try_from(dist).unwrap_or(u32::MAX)) / f64::from(longest_f);
        let phonetic = candidate.bytes().all(|b| b.is_ascii_alphabetic())
            && custom.key.bytes().all(|b| b.is_ascii_alphabetic());
        let score = if phonetic && soundex(candidate) == soundex(&custom.key) {
            raw_score * 0.3
        } else {
            raw_score
        };
        if score < FUZZY_MATCH_THRESHOLD && best.as_ref().is_none_or(|b| score < b.score) {
            best = Some(FuzzyMatch {
                word: custom.word.clone(),
                score,
                suffix: if keep_suffix {
                    suffix.to_owned()
                } else {
                    String::new()
                },
            });
        }
    }
    best
}

pub(crate) fn preserve_case(original: &str, replacement: &str) -> String {
    let letters: Vec<char> = original.chars().filter(|ch| ch.is_alphabetic()).collect();
    if !letters.is_empty() && letters.iter().all(|ch| ch.is_uppercase()) {
        return replacement.to_uppercase();
    }
    if let Some(first_alpha) = letters.first()
        && first_alpha.is_uppercase()
    {
        let mut chars = replacement.chars();
        if let Some(first) = chars.next() {
            return format!("{}{}", first.to_uppercase(), chars.as_str());
        }
    }
    replacement.to_owned()
}

fn soundex(value: &str) -> [u8; 4] {
    let upper = value.to_ascii_uppercase();
    let bytes = upper.as_bytes();
    let first = bytes.first().copied().unwrap_or(b'0');
    let code = |b: u8| -> u8 {
        match b {
            b'B' | b'F' | b'P' | b'V' => b'1',
            b'C' | b'G' | b'J' | b'K' | b'Q' | b'S' | b'X' | b'Z' => b'2',
            b'D' | b'T' => b'3',
            b'L' => b'4',
            b'M' | b'N' => b'5',
            b'R' => b'6',
            _ => b'0',
        }
    };
    let mut result = [b'0'; 4];
    result[0] = first;
    let mut len = 1_usize;
    let mut prev = code(first);
    for &b in bytes.iter().skip(1) {
        let next = code(b);
        if next != b'0' && next != prev && len < 4 {
            result[len] = next;
            len += 1;
        }
        prev = next;
    }
    result
}

fn edit_distance(left: &[u8], right: &[u8]) -> usize {
    let mut prev: Vec<usize> = (0..=right.len()).collect();
    let mut curr = vec![0_usize; right.len() + 1];
    for (i, &lb) in left.iter().enumerate() {
        curr[0] = i + 1;
        for (j, &rb) in right.iter().enumerate() {
            let cost = usize::from(lb != rb);
            curr[j + 1] = (curr[j] + 1).min(prev[j + 1] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[right.len()]
}

pub(crate) fn extract_word_tokens(text: &str) -> Vec<WordToken> {
    let mut tokens = Vec::new();
    let mut start: Option<usize> = None;
    for (idx, ch) in text.char_indices() {
        let is_token_char =
            (ch.is_alphanumeric() && !is_unspaced_cjk(ch)) || matches!(ch, '\'' | '\u{2019}' | '-');
        match (start, is_token_char) {
            (None, true) if ch.is_alphanumeric() && !is_unspaced_cjk(ch) => start = Some(idx),
            (Some(s), false) => {
                let raw = &text[s..idx];
                let trimmed = raw.trim_end_matches(['\'', '\u{2019}', '-']);
                if !trimmed.is_empty() {
                    tokens.push(WordToken {
                        start: s,
                        end: s + trimmed.len(),
                    });
                }
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        let trimmed = text[s..].trim_end_matches(['\'', '\u{2019}', '-']);
        if !trimmed.is_empty() {
            tokens.push(WordToken {
                start: s,
                end: s + trimmed.len(),
            });
        }
    }
    tokens
}

pub(crate) fn all_horizontal_ws_between(text: &str, slice: &[WordToken]) -> bool {
    slice.windows(2).all(|pair| {
        let gap = &text[pair[0].end..pair[1].start];
        !gap.is_empty() && gap.chars().all(|ch| ch == ' ' || ch == '\t')
    })
}

pub(crate) fn is_word_boundary_before(text: &str, byte_idx: usize) -> bool {
    text[..byte_idx]
        .chars()
        .next_back()
        .is_none_or(|ch| !is_word_char(ch))
}

pub(crate) fn is_word_boundary_after(text: &str, byte_idx: usize) -> bool {
    text[byte_idx..]
        .chars()
        .next()
        .is_none_or(|ch| !is_word_char(ch))
}

fn is_word_char(ch: char) -> bool {
    (ch.is_alphanumeric() || ch == '_') && !is_unspaced_cjk(ch)
}

fn is_unspaced_cjk(ch: char) -> bool {
    matches!(
        ch as u32,
        0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF
    )
}

pub(crate) fn advance_matching_chars(s: &str, char_count: usize) -> Option<usize> {
    if char_count == 0 {
        return Some(0);
    }
    s.char_indices()
        .nth(char_count - 1)
        .map(|(idx, ch)| idx + ch.len_utf8())
}

fn match_key(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_alphanumeric())
        .collect::<String>()
        .to_lowercase()
}

fn is_ascii_alnum(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|b| b.is_ascii_alphanumeric())
}

fn ends_with_possessive(value: &str) -> bool {
    let lower = value.to_lowercase();
    lower.ends_with("'s") || lower.ends_with("\u{2019}s")
}

fn split_possessive<'a>(phrase: &'a str, key: &'a str) -> (&'a str, &'a str) {
    for pat in ["'s", "'S", "\u{2019}s", "\u{2019}S"] {
        if let Some(suffix_start) = phrase.len().checked_sub(pat.len())
            && phrase.is_char_boundary(suffix_start)
            && &phrase[suffix_start..] == pat
            && key.len() > 1
        {
            return (&key[..key.len() - 1], &phrase[suffix_start..]);
        }
    }
    (key, "")
}

fn count_inner_possessives(value: &str) -> usize {
    value
        .split_whitespace()
        .filter(|part| {
            let lower = part.to_lowercase();
            lower.ends_with("'s") || lower.ends_with("\u{2019}s")
        })
        .count()
}
