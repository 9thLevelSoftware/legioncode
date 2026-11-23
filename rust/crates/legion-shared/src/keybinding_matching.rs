//! Keyboard shortcut matching across macOS Option glyphs and non-US `AltGr` layouts (`keybindingMatching.ts`).

use std::collections::BTreeSet;

/// Modifier keys held during a keyboard event (`meta`, `ctrl`, `shift`, `alt`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "Mirrors the four DOM KeyboardEvent modifier booleans from keybindingMatching.ts"
)]
pub struct ShortcutModifiers {
    /// Meta / Command key state.
    pub meta_key: bool,
    /// Control key state.
    pub ctrl_key: bool,
    /// Shift key state.
    pub shift_key: bool,
    /// Alt / Option key state.
    pub alt_key: bool,
}

/// Minimal keyboard event descriptor evaluated by [`shortcut_key_from_event`] and [`matches_keybinding_shortcut`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortcutEventLike<'a> {
    /// Physical key code (`KeyP`, `Digit1`, `BracketLeft`, ...), if available.
    pub code: Option<&'a str>,
    /// DOM `KeyboardEvent.key` string.
    pub key: &'a str,
    /// `true` when `AltGraph` modifier state is active on Windows/Linux.
    pub alt_graph: bool,
    /// Active modifier keys.
    pub modifiers: ShortcutModifiers,
}

/// Target parsed keybinding shortcut (`KeybindingShortcut`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeybindingShortcutLike<'a> {
    /// Normalized primary key (`"p"`, `"escape"`, `"["`, `"+"`, ...).
    pub key: &'a str,
    /// Required fixed modifier keys.
    pub modifiers: ShortcutModifiers,
    /// Platform-adaptive modifier (`Cmd` on macOS, `Ctrl` elsewhere).
    pub mod_key: bool,
}

fn event_code_shortcut_key(code: &str) -> Option<&'static str> {
    match code {
        "Backquote" => Some("`"),
        "Backslash" => Some("\\"),
        "BracketLeft" => Some("["),
        "BracketRight" => Some("]"),
        "Comma" => Some(","),
        "Digit0" => Some("0"),
        "Digit1" => Some("1"),
        "Digit2" => Some("2"),
        "Digit3" => Some("3"),
        "Digit4" => Some("4"),
        "Digit5" => Some("5"),
        "Digit6" => Some("6"),
        "Digit7" => Some("7"),
        "Digit8" => Some("8"),
        "Digit9" => Some("9"),
        "Equal" => Some("="),
        "Minus" => Some("-"),
        "Period" => Some("."),
        "Quote" => Some("'"),
        "Semicolon" => Some(";"),
        "Slash" => Some("/"),
        _ => None,
    }
}

// macOS US layout Option+letter glyphs (Option+P types "π"). Dead keys (E, I, N, U) are absent.
fn us_mac_option_glyph_letter(key: &str) -> Option<&'static str> {
    match key {
        "å" => Some("a"),
        "∫" => Some("b"),
        "ç" => Some("c"),
        "∂" => Some("d"),
        "ƒ" => Some("f"),
        "©" => Some("g"),
        "˙" => Some("h"),
        "∆" => Some("j"),
        "˚" => Some("k"),
        "¬" => Some("l"),
        "µ" => Some("m"),
        "ø" => Some("o"),
        "π" => Some("p"),
        "œ" => Some("q"),
        "®" => Some("r"),
        "ß" => Some("s"),
        "†" => Some("t"),
        "√" => Some("v"),
        "∑" => Some("w"),
        "≈" => Some("x"),
        "¥" => Some("y"),
        "Ω" => Some("z"),
        _ => None,
    }
}

/// Returns `true` for Apple platforms (`mac`, `iPhone`, `iPad`, `iPod`).
#[must_use]
pub fn is_mac_platform(platform: &str) -> bool {
    let lower = platform.to_ascii_lowercase();
    lower.contains("mac")
        || lower.contains("iphone")
        || lower.contains("ipad")
        || lower.contains("ipod")
}

/// Normalizes a DOM `KeyboardEvent.key` string (`"Esc"` -> `"escape"`, ASCII lowercase).
#[must_use]
pub fn normalize_event_key(key: &str) -> String {
    let lower = key.to_lowercase();
    if lower == "esc" {
        "escape".to_owned()
    } else {
        lower
    }
}

fn is_single_ascii_lower_letter(s: &str) -> bool {
    let bytes = s.as_bytes();
    bytes.len() == 1 && bytes[0].is_ascii_lowercase()
}

fn parse_key_code_letter(code: Option<&str>) -> Option<char> {
    let bytes = code?.as_bytes();
    if bytes.len() == 4 && bytes.starts_with(b"Key") && bytes[3].is_ascii_uppercase() {
        Some(bytes[3].to_ascii_lowercase() as char)
    } else {
        None
    }
}

/// Resolves the canonical shortcut key string represented by a keyboard event (`shortcutKeyFromEvent`).
#[must_use]
pub fn shortcut_key_from_event(
    key: &str,
    code: Option<&str>,
    shift_key: bool,
    alt_key: bool,
) -> String {
    let layout_key = normalize_event_key(key);
    if is_single_ascii_lower_letter(&layout_key) {
        return layout_key;
    }
    if alt_key
        && let Some(us_letter) = us_mac_option_glyph_letter(key)
        && parse_key_code_letter(code) == us_letter.chars().next()
    {
        return us_letter.to_owned();
    }
    let is_zoom_plus = layout_key == "+" && !shift_key && !alt_key;
    if is_zoom_plus && !code.is_some_and(|c| c.starts_with("Digit")) {
        return layout_key;
    }
    if let Some(physical) = code.and_then(event_code_shortcut_key) {
        return physical.to_owned();
    }
    layout_key
}

/// Returns the set of candidate shortcut keys matched by `event` (`resolveEventKeys`).
#[must_use]
pub fn resolve_event_keys(event: &ShortcutEventLike<'_>) -> BTreeSet<String> {
    let layout_key = normalize_event_key(event.key);
    let mut keys = BTreeSet::new();
    if let Some(letter) = parse_key_code_letter(event.code)
        && !is_single_ascii_lower_letter(&layout_key)
    {
        keys.insert(letter.to_string());
    }
    keys.insert(layout_key);
    keys.insert(shortcut_key_from_event(
        event.key,
        event.code,
        event.modifiers.shift_key,
        event.modifiers.alt_key,
    ));
    keys
}

/// Returns `true` when the held modifier keys match `shortcut` on `platform` (`matchesKeybindingShortcutModifiers`).
#[must_use]
pub fn matches_keybinding_shortcut_modifiers(
    modifiers: ShortcutModifiers,
    shortcut: &KeybindingShortcutLike<'_>,
    platform: &str,
) -> bool {
    let use_meta_for_mod = is_mac_platform(platform);
    let expected_meta = shortcut.modifiers.meta_key || (shortcut.mod_key && use_meta_for_mod);
    let expected_ctrl = shortcut.modifiers.ctrl_key || (shortcut.mod_key && !use_meta_for_mod);
    modifiers.meta_key == expected_meta
        && modifiers.ctrl_key == expected_ctrl
        && modifiers.shift_key == shortcut.modifiers.shift_key
        && modifiers.alt_key == shortcut.modifiers.alt_key
}

/// Returns `true` when `event` triggers `shortcut` on `platform` (`matchesKeybindingShortcut`).
#[must_use]
pub fn matches_keybinding_shortcut(
    event: &ShortcutEventLike<'_>,
    shortcut: &KeybindingShortcutLike<'_>,
    platform: &str,
) -> bool {
    if !is_mac_platform(platform)
        && event.alt_graph
        && !(event.key.len() == 1 && event.key.as_bytes()[0].is_ascii_alphanumeric())
    {
        return false;
    }
    if !matches_keybinding_shortcut_modifiers(event.modifiers, shortcut, platform) {
        return false;
    }
    resolve_event_keys(event).contains(shortcut.key)
}
