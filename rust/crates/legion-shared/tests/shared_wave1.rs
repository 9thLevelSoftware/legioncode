//! Unit tests for ported Wave 1 `legion-shared` modules (`keybinding_matching`, `markdown_chunks`, `markdown_task_edits`).

use legion_shared::keybinding_matching::{
    KeybindingShortcutLike, ShortcutEventLike, ShortcutModifiers, matches_keybinding_shortcut,
    shortcut_key_from_event,
};
use legion_shared::markdown_chunks::{
    LARGE_MARKDOWN_CHUNK_MAX_CHARS, split_markdown, with_markdown_definitions,
};
use legion_shared::markdown_task_edits::{set_markdown_task_checked, task_list_marker_offset};

#[test]
fn keybinding_matching_handles_mac_option_glyphs_and_altgr() {
    assert_eq!(shortcut_key_from_event("π", Some("KeyP"), false, true), "p");
    assert_eq!(
        shortcut_key_from_event("+", Some("BracketRight"), false, false),
        "+"
    );

    let shortcut = KeybindingShortcutLike {
        key: "p",
        modifiers: ShortcutModifiers {
            alt_key: true,
            ..Default::default()
        },
        mod_key: false,
    };
    let mac_opt_p = ShortcutEventLike {
        code: Some("KeyP"),
        key: "π",
        alt_graph: false,
        modifiers: ShortcutModifiers {
            alt_key: true,
            ..Default::default()
        },
    };
    assert!(matches_keybinding_shortcut(
        &mac_opt_p, &shortcut, "MacIntel"
    ));

    let win_altgr = ShortcutEventLike {
        code: Some("BracketRight"),
        key: "@",
        alt_graph: true,
        modifiers: ShortcutModifiers::default(),
    };
    let at_shortcut = KeybindingShortcutLike {
        key: "@",
        modifiers: ShortcutModifiers::default(),
        mod_key: false,
    };
    assert!(!matches_keybinding_shortcut(
        &win_altgr,
        &at_shortcut,
        "Win32"
    ));
}

#[test]
fn markdown_chunks_and_task_edits_match_ts() {
    assert_eq!(LARGE_MARKDOWN_CHUNK_MAX_CHARS, 128 * 1024);

    let doc = "---\ntitle: demo\n---\n\nFirst paragraph.\n\n[ref]: https://example.com\n\n# Next\n\nUse [ref].";
    let split = split_markdown(doc, 30);
    assert!(split.chunks.len() >= 2);
    assert_eq!(split.definitions, "[ref]: https://example.com");
    assert_eq!(
        with_markdown_definitions(&split.chunks[0].text, &split.definitions),
        format!("{}\n\n[ref]: https://example.com", split.chunks[0].text)
    );

    let md = "intro\n  - [ ] first task\n  - [x] second task";
    let start = md.find("  - [ ]").expect("item start");
    let offset = task_list_marker_offset(md, start).expect("marker offset");
    assert_eq!(&md[offset..offset + 3], "[ ]");
    assert_eq!(
        set_markdown_task_checked(md, offset, true),
        "intro\n  - [x] first task\n  - [x] second task"
    );
    assert_eq!(set_markdown_task_checked(md, 0, true), md);
}
