use std::collections::BTreeMap;

use proptest::prelude::*;
use t3_client_core::diff_markdown::Utf16Span;
use t3_edge::i18n::SUPPORTED_LOCALE_CODES;
use t3_surfaces::mobile_diff::{FontStyleFlags, SyntaxTokenInput};
use t3_web_ui::actions::{ProjectInput, RightPaneTab};
use t3_web_ui::{WebAppAction, WebAppEngine, escape_html};

#[test]
fn xss_payloads_are_escaped_across_all_surfaces_and_attrs() {
    let payload = r#""><script>alert(1)</script><img src=x onerror='alert(2)'>&"#;
    assert_eq!(
        escape_html(payload),
        "&quot;&gt;&lt;script&gt;alert(1)&lt;/script&gt;&lt;img src=x onerror=&#39;alert(2)&#39;&gt;&amp;"
    );

    let mut engine = WebAppEngine::new("en", 10);
    let _ = engine.dispatch(WebAppAction::SetPairingInput {
        value: payload.to_owned(),
    });
    let _ = engine.dispatch(WebAppAction::ApplyProjectsSnapshot {
        projects: vec![ProjectInput {
            id: payload.to_owned(),
            title: payload.to_owned(),
            workspace_root: payload.to_owned(),
            ..ProjectInput::default()
        }],
    });
    let _ = engine.dispatch(WebAppAction::CreateThread {
        project_id: payload.to_owned(),
        title: payload.to_owned(),
        model: None,
    });
    let _ = engine.dispatch(WebAppAction::UpdateComposerDraft {
        text: payload.to_owned(),
    });
    let _ = engine.dispatch(WebAppAction::AddContextPill {
        context_id: Some("pill-xss".to_owned()),
        screenshot_context_id: None,
        label: Some(payload.to_owned()),
        path: None,
    });
    let _ = engine.dispatch(WebAppAction::OpenCommandPalette);
    let _ = engine.dispatch(WebAppAction::SetCommandPaletteQuery {
        query: payload.to_owned(),
    });
    let chat_html = engine.render_html();
    assert!(!chat_html.contains("<script>"));
    assert!(!chat_html.contains("<img src=x"));
    assert!(chat_html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));

    // CSS injection in diff color must be stripped, and text escaped.
    let _ = engine.dispatch(WebAppAction::CloseCommandPalette);
    let _ = engine.dispatch(WebAppAction::DecorateDiffTokens {
        file_name: payload.to_owned(),
        tokens: vec![SyntaxTokenInput {
            content: payload.to_owned(),
            color: Some("red;background:url(javascript:alert(1))".to_owned()),
            font_style: FontStyleFlags::NONE,
            ligatures_enabled: false,
            attributes: BTreeMap::default(),
        }],
        word_highlights: vec![Utf16Span { start: 0, end: 4 }],
        highlight_background: Some("yellow;xss:1".to_owned()),
    });
    let diff_html = engine.render_html();
    assert!(!diff_html.contains("<script>"));
    assert!(!diff_html.contains("javascript:alert"));
    assert!(!diff_html.contains("xss:1"));

    // Terminal OSC title and cell XSS payload escaped.
    let _ = engine.dispatch(WebAppAction::SetRightTab {
        tab: RightPaneTab::Terminal,
    });
    let _ = engine.dispatch(WebAppAction::TerminalFeed {
        text: format!("\x1b]2;{payload}\x07{payload}"),
    });
    let term_html = engine.render_html();
    assert!(!term_html.contains("<script>"));
    assert!(!term_html.contains("<img src=x"));
}

proptest! {
    #[test]
    fn prop_arbitrary_xss_like_strings_never_emit_unescaped_script_or_img(
        raw in "[a-zA-Z0-9<>\"'&/=_ ()]{0,64}",
        tail in "[a-zA-Z0-9]{0,16}"
    ) {
        let evil = format!("<script>{raw}</script><img onerror={tail}>");
        let mut engine = WebAppEngine::new("en", 42);
        let _ = engine.dispatch(WebAppAction::ApplyProjectsSnapshot {
            projects: vec![ProjectInput {
                id: "p1".to_owned(),
                title: evil.clone(),
                workspace_root: evil.clone(),
                ..ProjectInput::default()
            }],
        });
        let _ = engine.dispatch(WebAppAction::UpdateComposerDraft { text: evil.clone() });
        let _ = engine.dispatch(WebAppAction::OpenCommandPalette);
        let _ = engine.dispatch(WebAppAction::SetCommandPaletteQuery { query: evil.clone() });
        let html = engine.render_html();
        prop_assert!(!html.contains("<script>"));
        prop_assert!(!html.contains("</script>"));
        prop_assert!(!html.contains("<img onerror="));
        prop_assert!(html.contains("&lt;script&gt;"));

        let _ = engine.dispatch(WebAppAction::SetRightTab { tab: RightPaneTab::Terminal });
        let _ = engine.dispatch(WebAppAction::TerminalFeed { text: format!("\x1b]2;{evil}\x07") });
        let term_html = engine.render_html();
        prop_assert!(!term_html.contains("<script>"));
        prop_assert!(!term_html.contains("<img onerror="));
    }

    #[test]
    fn prop_10_locale_resolution_always_lands_on_canonical_code(
        hint in "([a-zA-Z]{1,4}([_-][a-zA-Z]{1,4}){0,2})|.{0,12}"
    ) {
        let mut engine = WebAppEngine::new(&hint, 0);
        let code = engine.state().locale.catalog_locale.as_str();
        prop_assert!(SUPPORTED_LOCALE_CODES.contains(&code));
        let _ = engine.dispatch(WebAppAction::SetLocale {
            language: hint.clone(),
            system_locales: vec![hint],
        });
        let code2 = engine.state().locale.catalog_locale.as_str();
        prop_assert!(SUPPORTED_LOCALE_CODES.contains(&code2));
        let expected_attr = format!(r#"data-locale="{code2}""#);
        let rendered = engine.render_html();
        prop_assert!(rendered.contains(&expected_attr));
    }

    #[test]
    fn prop_terminal_feed_export_import_roundtrips_cleanly(
        text in "[\\x20-\\x7e\\r\\n]{0,120}"
    ) {
        let mut engine = WebAppEngine::new("en", 0);
        let _ = engine.dispatch(WebAppAction::TerminalFeed { text });
        let exported_ok = engine.try_dispatch(WebAppAction::TerminalExportSnapshot).is_ok();
        prop_assert!(exported_ok);
        let bytes = engine.state().terminal.last_exported_t3vt.clone();
        prop_assert!(bytes.is_some());
        if let Some(raw) = bytes {
            prop_assert!(raw.starts_with(b"T3VT"));
            let imported_ok = engine
                .try_dispatch(WebAppAction::TerminalImportSnapshot { bytes: raw })
                .is_ok();
            prop_assert!(imported_ok);
        }
    }

    #[test]
    fn prop_composer_whitespace_context_ids_never_create_pills(
        ws in "[ \\t\\r\\n]{0,16}"
    ) {
        let mut engine = WebAppEngine::new("en", 0);
        let _ = engine.dispatch(WebAppAction::AddContextPill {
            context_id: Some(ws),
            screenshot_context_id: None,
            label: Some("x".to_owned()),
            path: None,
        });
        prop_assert!(engine.state().composer.context_pills.is_empty());
    }
}
