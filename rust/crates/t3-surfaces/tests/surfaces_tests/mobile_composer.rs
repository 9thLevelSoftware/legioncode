//! BUG-23 integration tests: composer clipboard context normalization and fragment filtering.

use t3_surfaces::mobile_composer::{
    ComposerClipboardContext, ComposerClipboardRecord, build_clipboard_html,
    filter_clipboard_fragment_json, filter_clipboard_records,
};

#[test]
fn missing_empty_and_whitespace_ids_normalize_to_none_and_never_match() -> Result<(), String> {
    for candidate in [None, Some(""), Some("   "), Some("\t\n\r")] {
        let ctx = ComposerClipboardContext::new(candidate, candidate);
        assert_eq!(ctx.context_id(), None);
        assert_eq!(ctx.screenshot_context_id(), None);
        assert!(!ctx.has_context_id());
        assert_eq!(ctx.chip_pattern(), None);
        // Pasted Rust/JS/regex code full of "/)" must never match missing/blank IDs!
        assert!(!ctx.matches_text("const re = /abc/); return path/);"));
    }

    let deserialized: ComposerClipboardContext =
        serde_json::from_str(r#"{"contextId":"  \t ","screenshotContextId":""}"#)
            .map_err(|e| e.to_string())?;
    assert_eq!(deserialized.context_id(), None);
    assert_eq!(deserialized.screenshot_context_id(), None);
    assert_eq!(deserialized.chip_pattern(), None);

    let padded: ComposerClipboardContext =
        serde_json::from_str(r#"{"contextId":"  chip-42 ","screenshotContextId":" shot-99\n"}"#)
            .map_err(|e| e.to_string())?;
    assert_eq!(padded.context_id(), Some("chip-42"));
    assert_eq!(padded.screenshot_context_id(), Some("shot-99"));
    assert_eq!(padded.chip_pattern().as_deref(), Some("/chip-42)"));
    Ok(())
}

#[test]
fn pasted_code_containing_slash_paren_preserves_zero_unreferenced_chips() {
    let pasted_code = r#"
        fn parse(input: &str) -> bool {
            let re_like = "foo/) bar";
            input.ends_with("/)") || input.contains(re_like)
        }
    "#;
    let records = vec![
        ComposerClipboardRecord::new(None, None),
        ComposerClipboardRecord::new(Some(""), Some("")),
        ComposerClipboardRecord::new(Some("   \n "), Some(" \t ")),
        ComposerClipboardRecord::new(Some("ctx-1"), None),
        ComposerClipboardRecord::new(Some("ctx-2"), Some("shot-2")),
    ];
    let selected = filter_clipboard_records(pasted_code, &records);
    assert_eq!(selected, Vec::new());
}

#[test]
fn filter_selects_referenced_context_and_linked_screenshot_once_in_order() -> Result<(), String> {
    let raw_fragment = r#"{"version":1,"records":[
        {"contextId":null,"label":"stale-null"},
        {"contextId":"","label":"stale-empty"},
        {"contextId":"   ","label":"stale-ws"},
        {"contextId":"ctx-main","screenshotContextId":"ctx-shot","label":"Main file"},
        {"contextId":"ctx-shot","label":"Linked screenshot"},
        {"contextId":"ctx-unused","label":"Unmentioned"}
    ]}"#;
    let copied_text = "Look at /ctx-main) and /ctx-main) again plus /)";
    let filtered = filter_clipboard_fragment_json(copied_text, raw_fragment);

    assert_eq!(filtered.selected_records.len(), 2);
    assert_eq!(
        filtered.selected_records[0].context.context_id(),
        Some("ctx-main")
    );
    assert_eq!(
        filtered.selected_records[1].context.context_id(),
        Some("ctx-shot")
    );
    let fragment_json = filtered.fragment_json.ok_or("expected fragment_json")?;
    let parsed: serde_json::Value =
        serde_json::from_str(&fragment_json).map_err(|e| e.to_string())?;
    assert_eq!(parsed["version"].as_u64(), Some(1));
    let recs = parsed["records"]
        .as_array()
        .ok_or("missing records array")?;
    assert_eq!(recs.len(), 2);

    let html = filtered.html_fragment.ok_or("expected html_fragment")?;
    assert!(html.starts_with("<pre data-t3-context-fragment=\""));
    assert!(!html.contains("stale-null"));
    assert!(!html.contains("stale-empty"));
    assert!(!html.contains("Unmentioned"));
    Ok(())
}

#[test]
fn malformed_or_unmatched_fragment_drops_payload_without_corrupting_clipboard() {
    let unmatched =
        filter_clipboard_fragment_json("plain text /)", r#"{"records":[{"contextId":""}]}"#);
    assert_eq!(unmatched.selected_records, Vec::new());
    assert_eq!(unmatched.fragment_json, None);
    assert_eq!(unmatched.html_fragment, None);

    let invalid = filter_clipboard_fragment_json("/ctx-1)", "{not-json");
    assert_eq!(invalid.selected_records, Vec::new());
    assert_eq!(invalid.fragment_json, None);
    assert_eq!(invalid.html_fragment, None);

    let html = build_clipboard_html("a & b < c > \"d\"", r#"[{"x":"<a>"}]"#);
    assert_eq!(
        html,
        "<pre data-t3-context-fragment=\"%5B%7B%22x%22%3A%22%3Ca%3E%22%7D%5D\">a &amp; b &lt; c &gt; \"d\"</pre>"
    );
}
