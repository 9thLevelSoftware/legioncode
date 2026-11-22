//! `OnceLock<Regex>` query normalization, prefix stripping, and assistant citation helpers.

use std::borrow::Cow;
use std::sync::OnceLock;

use regex::Regex;

use crate::utf16::{fold_char_in_place, utf8_len_to_utf16, utf16_slice_safe};

/// Maximum length of an encoded `legion-citation://v1/...` URI per `assistantCitations.ts`.
pub const MAX_CITATION_HREF_LENGTH: usize = 9 * (8_000 + 8_000 + 3 * 4_096 + 2 * 32) + 1_000;

/// Maximum UTF-16 code units shown by [`assistant_citation_label`].
pub const ASSISTANT_CITATION_LABEL_MAX_UTF16: usize = 64;

fn wrapping_quotes_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let compiled = Regex::new(
            r#"^\s*["'`\u{201C}\u{201D}\u{2018}\u{2019}]+|["'`\u{201C}\u{201D}\u{2018}\u{2019}]+\s*$"#,
        );
        // INFALLIBLE: static ASCII/Unicode character-class pattern verified at compile-review time.
        compiled.expect("static wrapping-quotes regex is valid")
    })
}

fn whitespace_run_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let compiled = Regex::new(r"\s+");
        // INFALLIBLE: `\s+` is a minimal valid regex with no unbounded expansion.
        compiled.expect("static whitespace-run regex is valid")
    })
}

fn worktree_slash_prefix_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let compiled = Regex::new(r"^(?:legion|legioncode)/\S");
        // INFALLIBLE: static alternation prefix regex with fixed literal prefixes.
        compiled.expect("static worktree-slash prefix regex is valid")
    })
}

fn worktree_flat_prefix_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let compiled = Regex::new(r"^legion-[0-9a-f]{8}$");
        // INFALLIBLE: static fixed-length hex suffix pattern.
        compiled.expect("static worktree-flat prefix regex is valid")
    })
}

fn pr_query_prefix_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let compiled = Regex::new(r"^(?:pr|pull request|pull)\s*(#?\d)");
        // INFALLIBLE: static PR query prefix regex with single capture group.
        compiled.expect("static PR query prefix regex is valid")
    })
}

fn hash_query_prefix_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let compiled = Regex::new(r"^#\s*");
        // INFALLIBLE: `^#\s*` is a trivial anchored character match.
        compiled.expect("static hash query prefix regex is valid")
    })
}

fn flat_branch_query_prefix_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let compiled = Regex::new(r"(^| )legion-([0-9a-f]{1,8}(?: |$))");
        // INFALLIBLE: static bounded quantifier `{1,8}` hex branch prefix regex.
        compiled.expect("static flat-branch query prefix regex is valid")
    })
}

fn citation_link_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let compiled = Regex::new(r"\[Assistant quote\]\((legion-citation://v1/[^\s)]+)\)");
        // INFALLIBLE: static Markdown link regex matching `legion-citation://v1/` URLs without gigantic quantifiers.
        compiled.expect("static assistant citation link regex is valid")
    })
}

/// Returns `true` if `ch` is a Unicode combining mark stripped after NFC query folding.
#[inline]
const fn is_query_stripped_mark(ch: char) -> bool {
    let cp = ch as u32;
    matches!(
        cp,
        0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0x3099..=0x309A | 0xFE20..=0xFE2F
    )
}

/// Folds, trims wrapping quotes, and collapses whitespace in a user search query.
///
/// Matches `foldSearchQuery` from `packages/shared/src/paletteSearch.ts`.
///
/// # Examples
///
/// ```
/// use legion_shared::search::fold_search_query;
///
/// assert_eq!(fold_search_query("  \"Café  İstanbul\" "), "cafe istanbul");
/// assert_eq!(fold_search_query("´´´"), "");
/// ```
pub fn fold_search_query(query: &str) -> String {
    let unquoted = wrapping_quotes_re().replace_all(query, "");
    let mut folded = String::with_capacity(unquoted.len());
    for ch in unquoted.chars() {
        if is_query_stripped_mark(ch) {
            continue;
        }
        folded.push(fold_char_in_place(ch));
    }

    let mut out = String::with_capacity(folded.len());
    for token in folded.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(token);
    }
    out
}

/// Collapses any run of Unicode whitespace to a single ASCII space (`' '`).
pub fn normalize_whitespace(text: &str) -> Cow<'_, str> {
    whitespace_run_re().replace_all(text, " ")
}

/// Strips the app's own worktree branch prefix (`legion/`, `legioncode/`, or flat `legion-<8hex>`).
///
/// Matches `searchableBranch` from `paletteSearch.ts`.
///
/// # Examples
///
/// ```
/// use legion_shared::search::strip_worktree_branch_prefix;
///
/// assert_eq!(strip_worktree_branch_prefix("legion/cmdk-search"), "cmdk-search");
/// assert_eq!(strip_worktree_branch_prefix("legioncode/fix-ui"), "fix-ui");
/// assert_eq!(strip_worktree_branch_prefix("legion-0123abcd"), "0123abcd");
/// assert_eq!(strip_worktree_branch_prefix("legion/"), "legion/");
/// ```
pub fn strip_worktree_branch_prefix(branch: &str) -> &str {
    if worktree_slash_prefix_re().is_match(branch) {
        if let Some(rest) = branch.strip_prefix("legion/") {
            return rest;
        }
        if let Some(rest) = branch.strip_prefix("legioncode/") {
            return rest;
        }
    } else if worktree_flat_prefix_re().is_match(branch)
        && let Some(rest) = branch.strip_prefix("legion-")
    {
        return rest;
    }
    branch
}

/// Strips leading PR (`pr`, `pull request`, `#`) and worktree prefixes from a normalized query.
///
/// Matches `threadFieldQuery` from `paletteSearch.ts`.
pub fn strip_thread_query_prefixes(normalized_query: &str) -> String {
    let step1 = pr_query_prefix_re().replace(normalized_query, "$1");
    let step2 = hash_query_prefix_re().replace(&step1, "");
    let step3 = strip_worktree_branch_prefix(&step2);
    flat_branch_query_prefix_re()
        .replace_all(step3, "$1$2")
        .into_owned()
}

/// Extracts all candidate `legion-citation://v1/...` hrefs embedded as `[Assistant quote](...)` links.
///
/// # Examples
///
/// ```
/// use legion_shared::search::extract_assistant_citation_hrefs;
///
/// let md = "See [Assistant quote](legion-citation://v1/e/t/m?text=hi&start=0&end=2&prefix=&suffix=) here.";
/// assert_eq!(
///     extract_assistant_citation_hrefs(md),
///     vec!["legion-citation://v1/e/t/m?text=hi&start=0&end=2&prefix=&suffix="]
/// );
/// ```
pub fn extract_assistant_citation_hrefs(text: &str) -> Vec<&str> {
    citation_link_re()
        .captures_iter(text)
        .filter_map(|caps| {
            let href = caps.get(1)?.as_str();
            (href.len() <= MAX_CITATION_HREF_LENGTH).then_some(href)
        })
        .collect()
}

/// Collapses whitespace and truncates a citation preview to 64 UTF-16 code units without
/// splitting a surrogate pair, appending `'…'` when shortened.
///
/// Matches `assistantCitationLabel` from `assistantCitations.ts`.
///
/// # Examples
///
/// ```
/// use legion_shared::search::assistant_citation_label;
///
/// let input = format!("{}😀 and more", "a".repeat(63));
/// assert_eq!(assistant_citation_label(&input), format!("{}…", "a".repeat(63)));
/// ```
pub fn assistant_citation_label(raw_text: &str) -> String {
    let preview = normalize_whitespace(raw_text);
    if utf8_len_to_utf16(&preview) <= ASSISTANT_CITATION_LABEL_MAX_UTF16 {
        return preview.into_owned();
    }
    let truncated = utf16_slice_safe(&preview, 0, ASSISTANT_CITATION_LABEL_MAX_UTF16);
    format!("{truncated}…")
}
