use std::collections::BTreeMap;

use t3_client_core::diff_markdown::Utf16Span;
use t3_edge::i18n::{SUPPORTED_LOCALE_CODES, SUPPORTED_LOCALES};
use t3_surfaces::mobile_diff::{FontStyleFlags, SyntaxTokenInput};
use t3_terminal_vt::{AnsiPalette256, Rgba, TerminalGrid, encode_t3vt};
use t3_web_ui::actions::{PaletteOverrideInput, ProjectInput, RightPaneTab};
use t3_web_ui::{WebAppAction, WebAppEngine, WebHostEffect, emit_web_app_assets};

type TestRes = Result<(), Box<dyn std::error::Error>>;

#[test]
fn composer_bracketed_paste_and_context_pills_enforce_bug_23() {
    let mut engine = WebAppEngine::new("en", 100);
    let frag = r#"{"records":[
        {"contextId":"   ","label":"blank"},
        {"contextId":"","label":"empty"},
        {"contextId":"ctx-1","screenshotContextId":"shot-1","label":"src/lib.rs"},
        {"contextId":"shot-1","label":"screenshot.png"}
    ]}"#;
    let _ = engine.dispatch(WebAppAction::PasteIntoComposer {
        raw_text: "see \x1b[201~evil\x1b]0;pwn\x07/ctx-1)\r\nand code(a/)".to_owned(),
        clipboard_fragment_json: Some(frag.to_owned()),
    });

    assert_eq!(
        engine.state().composer.draft,
        "see evil/ctx-1)\nand code(a/)"
    );
    let pill_ids: Vec<_> = engine
        .state()
        .composer
        .context_pills
        .iter()
        .filter_map(|p| p.context.context_id())
        .collect();
    assert_eq!(pill_ids, vec!["ctx-1", "shot-1"]);

    // Whitespace-only manual AddContextPill normalizes to None and does not add a pill.
    let _ = engine.dispatch(WebAppAction::AddContextPill {
        context_id: Some("   ".to_owned()),
        screenshot_context_id: None,
        label: Some("ignored".to_owned()),
        path: None,
    });
    assert_eq!(engine.state().composer.context_pills.len(), 2);
}

#[test]
fn diff_rendering_preserves_utf16_word_highlights_and_font_styles_bug_22() {
    let mut engine = WebAppEngine::new("en", 100);
    let _ = engine.dispatch(WebAppAction::DecorateDiffTokens {
        file_name: "src/app.rs ".to_owned(),
        tokens: vec![SyntaxTokenInput {
            content: "let café = 42;".to_owned(),
            color: Some("#79c0ff".to_owned()),
            font_style: FontStyleFlags::BOLD | FontStyleFlags::ITALIC | FontStyleFlags::UNDERLINE,
            ligatures_enabled: false,
            attributes: BTreeMap::default(),
        }],
        word_highlights: vec![Utf16Span { start: 4, end: 8 }],
        highlight_background: Some("#4a3512".to_owned()),
    });

    assert_eq!(engine.state().right_tab, RightPaneTab::Diff);
    let runs = &engine.state().diff.files[0].lines[0].runs;
    assert_eq!(runs.len(), 3);
    assert!(runs[1].word_highlighted);
    assert_eq!(runs[1].text, "café");
    assert!(runs[1].font_style.is_bold());
    assert!(runs[1].font_style.is_italic());
    assert!(runs[1].font_style.is_underline());

    let html = engine.render_html();
    assert!(html.contains(r#"data-testid="diff-tab""#));
    assert!(html.contains(r#"data-testid="diff-word-highlight""#));
    assert!(html.contains("t3-bold t3-italic t3-underline t3-word-hl"));
    assert!(html.contains("src/app.rs "));
}

#[test]
fn unified_diff_load_populates_file_tabs_and_hunks() {
    let mut engine = WebAppEngine::new("en", 100);
    let patch = "\
diff --git a/foo.rs b/foo.rs
--- a/foo.rs
+++ b/foo.rs
@@ -1 +1 @@
-let old_val = 1;
+let new_val = 2;
";
    let _ = engine.dispatch(WebAppAction::LoadUnifiedDiff {
        patch: patch.to_owned(),
        switch_tab: true,
    });
    assert_eq!(engine.state().diff.files.len(), 1);
    assert_eq!(engine.state().diff.files[0].additions, 1);
    assert_eq!(engine.state().diff.files[0].deletions, 1);
    let html = engine.render_html();
    assert!(html.contains(r#"data-testid="diff-file-tab""#));
    assert!(html.contains("foo.rs"));
}

#[test]
fn vt_terminal_parser_and_sparse_ansi_palette_bug_25() -> TestRes {
    let mut engine = WebAppEngine::new("en", 100);
    let _ = engine.dispatch(WebAppAction::SetRightTab {
        tab: RightPaneTab::Terminal,
    });
    let _ = engine.dispatch(WebAppAction::TerminalFeed {
        text: "\x1b]2;Build Shell\x07\x1b[1;32mok\x1b[0m".to_owned(),
    });
    engine.try_dispatch(WebAppAction::TerminalApplyTheme {
        config_text: None,
        overrides: vec![
            PaletteOverrideInput {
                index: 1,
                color: "#ff1122".to_owned(),
            },
            PaletteOverrideInput {
                index: 196,
                color: "#cc0044".to_owned(),
            },
        ],
        foreground: Some("#eeeeee".to_owned()),
        background: Some("#101218".to_owned()),
        cursor_color: None,
    })?;
    assert_eq!(engine.state().terminal.palette.override_at(0), None);
    assert_eq!(
        engine.state().terminal.palette.override_at(1),
        Some(Rgba::rgb(0xff, 0x11, 0x22))
    );
    let html = engine.render_html();
    assert!(html.contains(r#"data-testid="terminal-grid""#));
    assert!(html.contains("Build Shell"));
    assert!(html.contains(r#"data-override-count="2""#));

    // Empty theme override resets prior slots rather than leaking stale colors (BUG-25).
    engine.try_dispatch(WebAppAction::TerminalApplyTheme {
        config_text: None,
        overrides: Vec::new(),
        foreground: None,
        background: None,
        cursor_color: None,
    })?;
    assert_eq!(engine.state().terminal.palette.override_at(1), None);
    assert_eq!(engine.state().terminal.palette.override_at(196), None);
    Ok(())
}

#[test]
fn vt_terminal_t3vt_export_import_and_corrupt_crc_rejected() -> TestRes {
    let mut engine = WebAppEngine::new("en", 100);
    let _ = engine.dispatch(WebAppAction::TerminalFeed {
        text: "\x1b[36mhello t3vt\x1b[0m".to_owned(),
    });
    engine.try_dispatch(WebAppAction::TerminalExportSnapshot)?;
    let exported = engine
        .state()
        .terminal
        .last_exported_t3vt
        .clone()
        .ok_or("missing exported bytes")?;
    assert!(exported.starts_with(b"T3VT"));

    // Reset grid then restore from binary T3VT snapshot.
    let _ = engine.dispatch(WebAppAction::TerminalReset);
    assert!(
        !engine
            .state()
            .terminal
            .viewport_text()
            .contains("hello t3vt")
    );

    engine.try_dispatch(WebAppAction::TerminalImportSnapshot {
        bytes: exported.clone(),
    })?;
    assert!(
        engine
            .state()
            .terminal
            .viewport_text()
            .contains("hello t3vt")
    );

    // Corrupt last CRC byte -> fails cleanly with InvalidSnapshot and surfaces error.
    let mut corrupt = exported;
    let last_idx = corrupt.len() - 1;
    corrupt[last_idx] ^= 0xFF;
    let err = engine.try_dispatch(WebAppAction::TerminalImportSnapshot { bytes: corrupt });
    assert!(err.is_err());
    assert!(engine.state().terminal.snapshot_error.is_some());

    // Also verify importing a v2 snapshot created externally with sparse palette.
    let mut grid = TerminalGrid::new(20, 4);
    grid.feed_str("ext");
    let mut pal = AnsiPalette256::new();
    pal.apply_theme_overrides(&[(2, Rgba::rgb(0, 200, 100))]);
    *grid.palette_mut() = pal;
    let ext_bytes = encode_t3vt(&grid.snapshot());
    engine.try_dispatch(WebAppAction::TerminalImportSnapshot { bytes: ext_bytes })?;
    assert!(engine.state().terminal.viewport_text().contains("ext"));
    assert_eq!(
        engine.state().terminal.palette.override_at(2),
        Some(Rgba::rgb(0, 200, 100))
    );
    Ok(())
}

#[test]
fn vt_terminal_paste_sanitizes_control_sequences_and_emits_rpc_frames() -> TestRes {
    let mut engine = WebAppEngine::new("en", 100);
    let _ = engine.dispatch(WebAppAction::TerminalFeed {
        text: "\x1b[?2004h".to_owned(),
    });
    let fx = engine.try_dispatch(WebAppAction::TerminalPaste {
        raw_text: "echo hi\x1b[201~\x1b]0;bad\x07\x00!".to_owned(),
    })?;
    assert_eq!(engine.state().terminal.input_draft, "echo hi!");
    assert!(matches!(
        fx.first(),
        Some(WebHostEffect::WsSendFrameParts { parts })
        if parts.iter().any(|p| p.contains("terminal.write") && p.contains(r"\u001b[200~echo hi!\u001b[201~"))
    ));
    Ok(())
}

#[test]
fn ctrl_k_command_palette_fuzzy_search_and_execute() {
    let mut engine = WebAppEngine::new("en", 100);
    let _ = engine.dispatch(WebAppAction::ApplyProjectsSnapshot {
        projects: vec![ProjectInput {
            id: "proj-auth".to_owned(),
            title: "Share auth across worktrees".to_owned(),
            workspace_root: "/repos/auth".to_owned(),
            ..ProjectInput::default()
        }],
    });
    let open_fx = engine.dispatch(WebAppAction::ToggleCommandPalette);
    assert!(engine.state().command_palette.open);
    assert!(matches!(
        open_fx.first(),
        Some(WebHostEffect::FocusSelector { selector }) if selector == r#"[data-testid="command-palette-input"]"#
    ));

    let _ = engine.dispatch(WebAppAction::SetCommandPaletteQuery {
        query: "auth".to_owned(),
    });
    assert!(
        !engine.state().command_palette.results.is_empty()
            || !engine.state().command_palette.query.is_empty()
    );
    assert_eq!(
        engine.state().command_palette.results[0].title,
        "Share auth across worktrees"
    );
    let html = engine.render_html();
    assert!(html.contains(r#"data-testid="command-palette-modal""#));
    assert!(html.contains(r#"<mark class="t3-palette-mark">auth</mark>"#));

    let _ = engine.dispatch(WebAppAction::ExecuteCommandPaletteItem { index: Some(0) });
    assert!(!engine.state().command_palette.open);
    assert_eq!(
        engine.state().active_project_id.as_deref(),
        Some("proj-auth")
    );
}

#[test]
fn all_10_locales_switch_and_preserve_bcp47_and_zh_tw_exclusion_bug_27() {
    assert_eq!(SUPPORTED_LOCALES.len(), 10);
    let mut engine = WebAppEngine::new("en", 100);
    for code in SUPPORTED_LOCALE_CODES {
        let _ = engine.dispatch(WebAppAction::SetLocale {
            language: (*code).to_owned(),
            system_locales: Vec::new(),
        });
        assert_eq!(engine.state().locale.catalog_locale, *code);
        let html = engine.render_html();
        assert!(html.contains(&format!(r#"data-locale="{code}""#)));
        assert!(html.contains(r#"data-testid="locale-selector""#));
    }

    // Swedish check (BUG-27 / BUG-24) and Traditional Chinese exclusion.
    let _ = engine.dispatch(WebAppAction::SetLocale {
        language: "system".to_owned(),
        system_locales: vec!["zh-TW".to_owned(), "sv-SE".to_owned()],
    });
    assert_eq!(engine.state().locale.catalog_locale, "sv");
    assert!(engine.render_html().contains("Parkoppling"));

    let _ = engine.dispatch(WebAppAction::SetLocale {
        language: "system".to_owned(),
        system_locales: vec!["zh-Hant-TW".to_owned()],
    });
    assert_eq!(engine.state().locale.catalog_locale, "en");
}

#[test]
fn static_bundle_assets_are_deterministic_and_thin_glue() {
    let a = emit_web_app_assets();
    let b = emit_web_app_assets();
    assert_eq!(a, b);
    assert_eq!(a.files().len(), 3);
    assert!(a.index_html.contains(r#"src="./app.mjs""#));
    assert!(a.index_html.contains(r#"href="./styles.css""#));
    assert!(a.styles_css.contains(".t3-vt-cursor"));
    assert!(a.app_mjs.contains(r#"from "./t3_wasm_ffi.js""#));
    assert!(a.app_mjs.contains("WasmWebApp"));
    assert!(a.app_mjs.contains("/api/auth/pair"));
    assert!(a.app_mjs.contains("/api/projects"));
    assert!(a.app_mjs.contains("WebSocket"));
    assert!(a.app_mjs.contains("sessionStorage"));
    let lines = a.app_mjs.lines().count();
    assert!(
        (100..=140).contains(&lines),
        "expected ~100-140 lines, got {lines}"
    );
}

#[test]
fn dispatch_json_and_state_snapshot_json_roundtrip() -> TestRes {
    let mut engine = WebAppEngine::new("ja", 500);
    let out = engine.dispatch_json(r#"{"type":"setRightTab","tab":"terminal"}"#)?;
    let parsed: serde_json::Value = serde_json::from_str(&out)?;
    assert_eq!(parsed["effects"], serde_json::json!([]));
    assert!(
        parsed["html"]
            .as_str()
            .is_some_and(|h| h.contains(r#"data-testid="terminal-grid""#))
    );
    assert_eq!(parsed["state"]["rightTab"], "terminal");
    assert_eq!(parsed["state"]["locale"]["catalogLocale"], "ja");

    let snap_raw = engine.state_snapshot_json()?;
    let snap: serde_json::Value = serde_json::from_str(&snap_raw)?;
    assert_eq!(snap["locale"]["catalogLocale"], "ja");
    Ok(())
}
