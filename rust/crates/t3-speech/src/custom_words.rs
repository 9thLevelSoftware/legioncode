//! Custom speech vocabulary, recognition hints, and safe project-name casing (`d20e6c294c` #39884).
//!
//! Mirrors `packages/shared/src/speech.ts` (`resolveSpeechVocabulary`) and
//! `apps/server/src/speech/customWords.ts` (`makeSpeechTextCorrector`, `speechRecognitionWords`,
//! `normalizeSpeechCustomWords`):
//! - Recognition-only hints (`speech_hints`) travel apart from `speech_custom_words` so project
//!   names never rewrite ordinary spoken words in the transcript.
//! - When generating or applying optional project-name recasing hints, ordinary English
//!   dictionary words (`"code"`, `"web"`, `"app"`, `"main"`, `"server"`, etc.) are never recased,
//!   and only tokens with distinctive mixed/upper casing (such as `"T3Code"`, `"OpenAI"`,
//!   `"GitHub"`, `"macOS"`) are eligible.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

pub use crate::custom_words_dict::{
    apply_project_name_hints, has_distinctive_project_casing, is_ordinary_english_word,
    project_name_recase_hints,
};
use crate::custom_words_match::{
    CustomWordKey, FuzzyMatch, advance_matching_chars, all_horizontal_ws_between, best_fuzzy_match,
    build_custom_word_keys, extract_word_tokens, is_word_boundary_after, is_word_boundary_before,
    preserve_case,
};
use crate::types::{SpeechCustomWord, TranscriptionOptions};

/// Maximum number of custom vocabulary entries forwarded on the wire (`speech.ts`).
pub const MAX_SPEECH_CUSTOM_WORDS: usize = 100;
/// Maximum number of recognition-only hints (`packages/contracts/src/speech.ts`).
pub const MAX_SPEECH_HINTS: usize = 8;
/// Maximum character length of a normalized term, alias, or hint.
pub const MAX_SPEECH_WORD_LEN: usize = 50;
/// Maximum aliases retained per custom word entry.
pub const MAX_SPEECH_ALIASES_PER_WORD: usize = 8;

/// Separated custom vocabulary dictionary and recognition-only hints (`#39884`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedSpeechVocabulary {
    /// Custom dictionary entries allowed to rewrite spoken transcripts.
    pub speech_custom_words: Vec<SpeechCustomWord>,
    /// Recognition-only context hints forwarded to providers without rewriting transcripts.
    pub speech_hints: Vec<String>,
}

/// Resolves personal and project dictionaries while keeping `hints` out of transcript rewrites.
#[must_use]
pub fn resolve_speech_vocabulary(
    personal: &[SpeechCustomWord],
    project: &[SpeechCustomWord],
    hints: &[String],
) -> ResolvedSpeechVocabulary {
    let mut seen_dict = HashSet::new();
    let mut speech_custom_words = Vec::new();
    for entry in project.iter().chain(personal.iter()) {
        if speech_custom_words.len() >= MAX_SPEECH_CUSTOM_WORDS {
            break;
        }
        let term = collapse_whitespace(&entry.term);
        let term_key = vocabulary_key(&term);
        if term.is_empty()
            || term.chars().count() > MAX_SPEECH_WORD_LEN
            || !seen_dict.insert(term_key)
        {
            continue;
        }
        let mut unique_aliases = Vec::new();
        for alias in &entry.aliases {
            if unique_aliases.len() >= MAX_SPEECH_ALIASES_PER_WORD {
                break;
            }
            let normalized = collapse_whitespace(alias);
            let alias_key = vocabulary_key(&normalized);
            if !normalized.is_empty()
                && normalized.chars().count() <= MAX_SPEECH_WORD_LEN
                && seen_dict.insert(alias_key)
            {
                unique_aliases.push(normalized);
            }
        }
        speech_custom_words.push(SpeechCustomWord {
            term,
            aliases: unique_aliases,
        });
    }

    let mut known: HashSet<String> = speech_custom_words
        .iter()
        .flat_map(|entry| {
            std::iter::once(&entry.term)
                .chain(entry.aliases.iter())
                .map(|s| vocabulary_key(s))
        })
        .collect();

    let mut speech_hints = Vec::new();
    for hint in hints {
        if speech_hints.len() >= MAX_SPEECH_HINTS {
            break;
        }
        let term = collapse_whitespace(hint);
        let key = vocabulary_key(&term);
        if term.is_empty() || term.chars().count() > MAX_SPEECH_WORD_LEN || !known.insert(key) {
            continue;
        }
        speech_hints.push(term);
    }

    ResolvedSpeechVocabulary {
        speech_custom_words,
        speech_hints,
    }
}

/// Normalizes a custom word list (`normalizeSpeechCustomWords` in `customWords.ts`).
#[must_use]
pub fn normalize_speech_custom_words(words: &[SpeechCustomWord]) -> Vec<SpeechCustomWord> {
    resolve_speech_vocabulary(&[], words, &[]).speech_custom_words
}

/// Orders recognition hints before dictionary terms, capped at 100 entries (`speechRecognitionWords`).
#[must_use]
pub fn speech_recognition_words(hints: &[String], terms: &[String]) -> Vec<String> {
    hints
        .iter()
        .chain(terms.iter())
        .take(MAX_SPEECH_CUSTOM_WORDS)
        .cloned()
        .collect()
}

/// Post-processes transcript text using explicit dictionary spellings/aliases and fuzzy terms
/// (`makeSpeechTextCorrector` in `apps/server/src/speech/customWords.ts`).
#[derive(Debug, Clone)]
pub struct SpeechTextCorrector {
    fuzzy_keys: Vec<CustomWordKey>,
    spellings: Vec<(String, String)>,
    lookup: HashMap<String, String>,
}

impl SpeechTextCorrector {
    /// Builds a corrector from fuzzy `words` and exact `dictionary` term/alias pairs.
    #[must_use]
    pub fn new(words: &[String], dictionary: &[SpeechCustomWord]) -> Self {
        let fuzzy_keys = build_custom_word_keys(words);
        let mut pairs: Vec<(String, String)> = dictionary
            .iter()
            .flat_map(|entry| {
                std::iter::once(&entry.term)
                    .chain(entry.aliases.iter())
                    .filter(|s| !s.is_empty())
                    .map(|spelling| (spelling.clone(), entry.term.clone()))
            })
            .collect();
        pairs.sort_by_key(|a| std::cmp::Reverse(a.0.chars().count()));
        let mut lookup = HashMap::new();
        for (spelling, term) in &pairs {
            lookup
                .entry(spelling.to_lowercase())
                .or_insert_with(|| term.clone());
        }
        Self {
            fuzzy_keys,
            spellings: pairs,
            lookup,
        }
    }

    /// Builds a corrector from [`TranscriptionOptions`], never rewriting transcripts from
    /// `speech_hints` (`#39884`).
    #[must_use]
    pub fn from_options(options: &TranscriptionOptions) -> Self {
        let normalized = normalize_speech_custom_words(&options.speech_custom_words);
        let terms: Vec<String> = normalized.iter().map(|entry| entry.term.clone()).collect();
        Self::new(&terms, &normalized)
    }

    /// Rewrites matching aliases and fuzzy custom terms inside `text`.
    #[must_use]
    pub fn apply(&self, text: &str) -> String {
        if self.spellings.is_empty() {
            return self.apply_fuzzy(text);
        }
        let mut out = String::with_capacity(text.len());
        let mut cursor = 0_usize;
        let mut scan = 0_usize;
        while scan < text.len() {
            if is_word_boundary_before(text, scan)
                && let Some((matched_len, key)) = self.find_longest_spelling_at(&text[scan..])
                && is_word_boundary_after(text, scan + matched_len)
            {
                out.push_str(&self.apply_fuzzy(&text[cursor..scan]));
                let replacement = self
                    .lookup
                    .get(&key)
                    .map_or(&text[scan..scan + matched_len], String::as_str);
                out.push_str(replacement);
                scan += matched_len;
                cursor = scan;
                continue;
            }
            let step = text[scan..].chars().next().map_or(1, char::len_utf8);
            scan += step;
        }
        out.push_str(&self.apply_fuzzy(&text[cursor..]));
        out
    }

    fn find_longest_spelling_at(&self, rest: &str) -> Option<(usize, String)> {
        let rest_lower = rest.to_lowercase();
        for (spelling, _) in &self.spellings {
            let sp_lower = spelling.to_lowercase();
            if rest_lower.starts_with(&sp_lower) {
                let matched_len = advance_matching_chars(rest, spelling.chars().count())?;
                return Some((matched_len, sp_lower));
            }
        }
        None
    }

    fn apply_fuzzy(&self, text: &str) -> String {
        if self.fuzzy_keys.is_empty() || text.is_empty() {
            return text.to_owned();
        }
        let tokens = extract_word_tokens(text);
        if tokens.is_empty() {
            return text.to_owned();
        }
        let mut out = String::with_capacity(text.len());
        let mut cursor = 0_usize;
        let mut idx = 0_usize;
        while idx < tokens.len() {
            let max_span = 4_usize.min(tokens.len() - idx);
            let mut best: Option<(usize, FuzzyMatch)> = None;
            for count in (1..=max_span).rev() {
                let slice = &tokens[idx..idx + count];
                if !all_horizontal_ws_between(text, slice) {
                    continue;
                }
                let phrase = &text[slice[0].start..slice[count - 1].end];
                if let Some(m) = best_fuzzy_match(phrase, &self.fuzzy_keys)
                    && best.as_ref().is_none_or(|(bc, bm)| {
                        m.score < bm.score
                            || ((m.score - bm.score).abs() < f64::EPSILON && count < *bc)
                    })
                {
                    best = Some((count, m));
                }
            }
            if let Some((count, m)) = best {
                let first = &tokens[idx];
                out.push_str(&text[cursor..first.start]);
                out.push_str(&preserve_case(&text[first.start..first.end], &m.word));
                out.push_str(&m.suffix);
                cursor = tokens[idx + count - 1].end;
                idx += count;
            } else {
                idx += 1;
            }
        }
        out.push_str(&text[cursor..]);
        out
    }
}

/// Convenience wrapper applying dictionary term/alias replacements to `text`.
#[must_use]
pub fn apply_speech_aliases(text: &str, words: &[SpeechCustomWord]) -> String {
    SpeechTextCorrector::new(&[], words).apply(text)
}

fn collapse_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn vocabulary_key(value: &str) -> String {
    value.trim().to_lowercase()
}
