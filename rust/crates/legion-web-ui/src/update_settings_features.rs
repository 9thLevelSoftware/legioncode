//! Pure Safe Rust helpers for `composerEmpty` keybinding evaluation/search (`342b58a79f`,
//! `3d31ffcd9a`) and compact sidebar update capsule countdowns.

use legion_shared::text_norm::accent_folded_contains;

use crate::state::WebAppState;
use crate::state_features::{KeybindingEntry, KeybindingWhenContext, UpdateCapsulePhase};

/// Returns the keybinding `when` context derived from the current [`WebAppState`].
#[must_use]
pub fn keybinding_context(state: &WebAppState) -> KeybindingWhenContext {
    KeybindingWhenContext {
        composer_empty: state.composer.draft.trim().is_empty()
            && state.composer.context_pills.is_empty(),
        composer_focus: state.composer.focused,
        dictating: state.composer.dictating,
        terminal_focus: state.features.terminal_focused,
        diff_open: !state.diff.files.is_empty(),
        modal_open: state.command_palette.open
            || state.features.repo_picker.open
            || state.features.environments_map.open,
    }
}

/// Evaluates keybinding `when` expressions supporting `!`, `&&`, `||`, parentheses, and
/// `composerEmpty` / `composerFocus` / `dictating` / `terminalFocus` / `diffOpen` / `modalOpen`.
#[must_use]
pub fn evaluate_when_condition(when_expr: Option<&str>, ctx: KeybindingWhenContext) -> bool {
    let Some(raw) = when_expr.map(str::trim).filter(|s| !s.is_empty()) else {
        return true;
    };
    let tokens = tokenize_when(raw);
    let mut pos = 0_usize;
    parse_or(&tokens, &mut pos, ctx)
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum WhenTok {
    Not,
    And,
    Or,
    LParen,
    RParen,
    Ident(String),
}

fn tokenize_when(input: &str) -> Vec<WhenTok> {
    let mut out = Vec::new();
    let bytes = input.as_bytes();
    let mut i = 0_usize;
    while i < bytes.len() {
        match bytes[i] {
            b' ' | b'\t' | b'\r' | b'\n' => i += 1,
            b'!' => {
                out.push(WhenTok::Not);
                i += 1;
            }
            b'(' => {
                out.push(WhenTok::LParen);
                i += 1;
            }
            b')' => {
                out.push(WhenTok::RParen);
                i += 1;
            }
            b'&' if i + 1 < bytes.len() && bytes[i + 1] == b'&' => {
                out.push(WhenTok::And);
                i += 2;
            }
            b'|' if i + 1 < bytes.len() && bytes[i + 1] == b'|' => {
                out.push(WhenTok::Or);
                i += 2;
            }
            _ => {
                let start = i;
                while i < bytes.len()
                    && !bytes[i].is_ascii_whitespace()
                    && !matches!(bytes[i], b'!' | b'(' | b')' | b'&' | b'|')
                {
                    i += 1;
                }
                out.push(WhenTok::Ident(input[start..i].to_owned()));
            }
        }
    }
    out
}

fn parse_or(toks: &[WhenTok], pos: &mut usize, ctx: KeybindingWhenContext) -> bool {
    let mut left = parse_and(toks, pos, ctx);
    while toks.get(*pos) == Some(&WhenTok::Or) {
        *pos += 1;
        let right = parse_and(toks, pos, ctx);
        left = left || right;
    }
    left
}

fn parse_and(toks: &[WhenTok], pos: &mut usize, ctx: KeybindingWhenContext) -> bool {
    let mut left = parse_unary(toks, pos, ctx);
    while toks.get(*pos) == Some(&WhenTok::And) {
        *pos += 1;
        let right = parse_unary(toks, pos, ctx);
        left = left && right;
    }
    left
}

fn parse_unary(toks: &[WhenTok], pos: &mut usize, ctx: KeybindingWhenContext) -> bool {
    if toks.get(*pos) == Some(&WhenTok::Not) {
        *pos += 1;
        return !parse_unary(toks, pos, ctx);
    }
    if toks.get(*pos) == Some(&WhenTok::LParen) {
        *pos += 1;
        let val = parse_or(toks, pos, ctx);
        if toks.get(*pos) == Some(&WhenTok::RParen) {
            *pos += 1;
        }
        return val;
    }
    match toks.get(*pos) {
        Some(WhenTok::Ident(name)) => {
            *pos += 1;
            match name.as_str() {
                "composerEmpty" => ctx.composer_empty,
                "composerFocus" | "composerFocused" => ctx.composer_focus,
                "dictating" => ctx.dictating,
                "terminalFocus" => ctx.terminal_focus,
                "diffOpen" => ctx.diff_open,
                "modalOpen" => ctx.modal_open,
                "true" => true,
                _ => false,
            }
        }
        _ => false,
    }
}

/// Filters keybinding entries by accent-folded query across ID, label, shortcut, category,
/// and `when` clause (`3d31ffcd9a`).
#[must_use]
pub fn filter_keybindings<'a>(
    entries: &'a [KeybindingEntry],
    query: &str,
) -> Vec<&'a KeybindingEntry> {
    let needle = query.trim();
    if needle.is_empty() {
        return entries.iter().collect();
    }
    entries
        .iter()
        .filter(|entry| {
            accent_folded_contains(&entry.command_id, needle)
                || accent_folded_contains(&entry.label, needle)
                || accent_folded_contains(&entry.shortcut, needle)
                || accent_folded_contains(&entry.category, needle)
                || entry
                    .when_expr
                    .as_deref()
                    .is_some_and(|w| accent_folded_contains(w, needle))
        })
        .collect()
}

/// Advances the compact sidebar update capsule countdown from `now_ms`.
pub fn tick_update_countdown(state: &mut WebAppState, now_ms: i64) {
    let cap = &mut state.features.update_capsule;
    if cap.phase != UpdateCapsulePhase::Countdown {
        return;
    }
    let Some(restart_at) = cap.restart_at_ms else {
        return;
    };
    let remaining_ms = restart_at.saturating_sub(now_ms);
    if remaining_ms <= 0 {
        cap.countdown_seconds = 0;
        cap.phase = UpdateCapsulePhase::Installing;
    } else {
        let secs = (remaining_ms + 999) / 1000;
        cap.countdown_seconds = u32::try_from(secs).unwrap_or(u32::MAX);
    }
}
