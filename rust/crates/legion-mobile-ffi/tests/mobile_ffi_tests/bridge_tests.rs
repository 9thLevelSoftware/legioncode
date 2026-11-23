//! Safe Rust bridge tests covering `BUG-22`..`BUG-26` and `"LCVT"` snapshot round-trips.

use legion_edge::i18n::SUPPORTED_LOCALE_CODES;
use legion_mobile_ffi::core::{
    DecorateDiffResponse, HostSwapChunksResponse, PaletteResolveResponse, ThemePlanResponse,
    VtDecodedSnapshotResponse, decorate_diff_json, filter_clipboard_json, host_swap_chunks_json,
    live_activity_json, nav_strings_json, palette_resolve_json, theme_plan_json,
    vt_decode_snapshot_json, vt_render_snapshot,
};
use legion_surfaces::mobile_composer::ClipboardFilterResult;
use legion_surfaces::mobile_diff::FontStyleFlags;
use legion_surfaces::mobile_i18n::{AgentLiveActivityPresentation, MobileNavigationStrings};
use legion_surfaces::mobile_tunnel::HttpProxyPhase;
use legion_terminal_vt::palette::xterm_256_rgb;

#[test]
fn bug22_diff_decoration_preserves_font_styles_utf16_and_non_color_attrs() -> Result<(), String> {
    let input = r##"{
        "tokens": [
            {
                "content": "👋fn ",
                "color": "#ff00aa",
                "fontStyle": 3,
                "ligaturesEnabled": false,
                "attributes": { "customLink": "https://example.test", "role": "kw" }
            },
            {
                "content": "name()",
                "color": "#112233",
                "fontStyle": 12,
                "ligaturesEnabled": true
            }
        ],
        "wordHighlights": [{ "start": 2, "end": 7 }],
        "highlightBackground": "#445566"
    }"##;

    let out_json = decorate_diff_json(input).map_err(|err| err.to_string())?;
    let resp: DecorateDiffResponse =
        serde_json::from_str(&out_json).map_err(|err| err.to_string())?;

    assert_eq!(resp.runs.len(), 4);
    assert_eq!(resp.runs[0].text, "👋");
    assert!(!resp.runs[0].word_highlighted);
    assert_eq!(resp.runs[0].background_color, None);
    assert_eq!(
        resp.runs[0].font_style,
        FontStyleFlags::BOLD | FontStyleFlags::ITALIC
    );

    assert_eq!(resp.runs[1].text, "fn ");
    assert!(resp.runs[1].word_highlighted);
    assert_eq!(resp.runs[1].background_color.as_deref(), Some("#445566"));
    assert_eq!(
        resp.runs[1].font_style,
        FontStyleFlags::BOLD | FontStyleFlags::ITALIC
    );
    assert_eq!(
        resp.runs[1]
            .attributes
            .get("customLink")
            .map(String::as_str),
        Some("https://example.test")
    );

    assert_eq!(resp.runs[2].text, "na");
    assert!(resp.runs[2].word_highlighted);
    assert_eq!(
        resp.runs[2].font_style,
        FontStyleFlags::UNDERLINE | FontStyleFlags::STRIKETHROUGH
    );
    assert!(resp.runs[2].ligatures_enabled);

    assert_eq!(resp.runs[3].text, "me()");
    assert!(!resp.runs[3].word_highlighted);
    assert_eq!(resp.runs[3].background_color, None);
    Ok(())
}

#[test]
fn bug23_clipboard_filter_normalizes_blank_ids_and_keeps_linked_screenshots() -> Result<(), String>
{
    let code_only = r#"{
        "text": "const re = /abc/); return path/);",
        "records": [
            { "contextId": null, "label": "null-id" },
            { "contextId": "", "label": "empty-id" },
            { "contextId": "   \t\n ", "label": "ws-id" },
            { "contextId": "ctx-unused", "label": "unused" }
        ]
    }"#;
    let empty_out = filter_clipboard_json(code_only).map_err(|err| err.to_string())?;
    let empty_parsed: ClipboardFilterResult =
        serde_json::from_str(&empty_out).map_err(|err| err.to_string())?;
    assert_eq!(empty_parsed.selected_records, Vec::new());
    assert_eq!(empty_parsed.fragment_json, None);
    assert_eq!(empty_parsed.html_fragment, None);

    let linked = r#"{
        "text": "Check /ctx-main) alongside /)",
        "fragmentJson": "{\"version\":1,\"records\":[{\"contextId\":\"\",\"label\":\"blank\"},{\"contextId\":\"ctx-main\",\"screenshotContextId\":\"ctx-shot\",\"label\":\"Main\"},{\"contextId\":\"ctx-shot\",\"label\":\"Shot\"}]}"
    }"#;
    let linked_out = filter_clipboard_json(linked).map_err(|err| err.to_string())?;
    let linked_parsed: ClipboardFilterResult =
        serde_json::from_str(&linked_out).map_err(|err| err.to_string())?;
    assert_eq!(linked_parsed.selected_records.len(), 2);
    assert_eq!(
        linked_parsed.selected_records[0].context.context_id(),
        Some("ctx-main")
    );
    assert_eq!(
        linked_parsed.selected_records[1].context.context_id(),
        Some("ctx-shot")
    );
    assert!(
        linked_parsed
            .html_fragment
            .as_deref()
            .is_some_and(|html| html.starts_with("<pre data-legion-context-fragment=\""))
    );
    Ok(())
}

#[test]
fn bug24_nav_and_live_activity_translate_all_locales_without_english_leakage() -> Result<(), String>
{
    let en_nav: MobileNavigationStrings =
        serde_json::from_str(&nav_strings_json("en").map_err(|err| err.to_string())?)
            .map_err(|err| err.to_string())?;

    for code in SUPPORTED_LOCALE_CODES {
        if *code == "en" {
            continue;
        }
        let nav: MobileNavigationStrings =
            serde_json::from_str(&nav_strings_json(code).map_err(|err| err.to_string())?)
                .map_err(|err| err.to_string())?;
        assert_eq!(nav.locale, *code);
        assert_ne!(nav.settings, en_nav.settings);
        assert_ne!(nav.new_thread, en_nav.new_thread);
    }

    let tw_nav: MobileNavigationStrings =
        serde_json::from_str(&nav_strings_json("zh-TW").map_err(|err| err.to_string())?)
            .map_err(|err| err.to_string())?;
    assert_eq!(tw_nav.locale, "en");

    let sys_nav: serde_json::Value = serde_json::from_str(
        &nav_strings_json(r#"{"language":"system","systemLocales":["zh-TW","de-DE"]}"#)
            .map_err(|err| err.to_string())?,
    )
    .map_err(|err| err.to_string())?;
    assert_eq!(sys_nav["locale"].as_str(), Some("de"));
    assert_eq!(sys_nav["formattingLocale"].as_str(), Some("de-DE"));

    let act_json = live_activity_json(
        r#"{"locale":"ja","active":false,"rows":[{"phase":"completed","title":"Build","project":"Core"},{"phase":"failed","title":"Lint","project":"Mobile"}],"phase":"failed"}"#,
    )
    .map_err(|err| err.to_string())?;
    let act: AgentLiveActivityPresentation =
        serde_json::from_str(&act_json).map_err(|err| err.to_string())?;
    assert_eq!(act.locale, "ja");
    assert_eq!(act.action, None);
    assert_eq!(act.chip, "失敗");
    assert_eq!(act.summary, "完了, 1 件が失敗");
    Ok(())
}

#[test]
fn bug25_palette_reset_and_lcvt_snapshot_round_trip() -> Result<(), String> {
    let sparse = palette_resolve_json(
        r#"{"themeConfig":"foreground = #d1d5da\npalette = 1=#ff2e3f\npalette = 196=#ff1122\n","indices":[0,1,2,196]}"#,
    )
    .map_err(|err| err.to_string())?;
    let sparse_resp: PaletteResolveResponse =
        serde_json::from_str(&sparse).map_err(|err| err.to_string())?;
    assert_eq!(sparse_resp.override_count, 2);
    assert!(!sparse_resp.resolved[0].is_override);
    assert_eq!(
        (
            sparse_resp.resolved[0].color.r,
            sparse_resp.resolved[0].color.g,
            sparse_resp.resolved[0].color.b
        ),
        (xterm_256_rgb(0).r, xterm_256_rgb(0).g, xterm_256_rgb(0).b)
    );
    assert!(sparse_resp.resolved[1].is_override);
    assert_eq!(sparse_resp.resolved[1].color.hex, "#ff2e3f");
    assert!(!sparse_resp.resolved[2].is_override);

    // Switching to an empty palette clears all prior slots instead of retaining stale colors.
    let reset = palette_resolve_json(
        r##"{"priorOverrides":[{"index":1,"color":"#ff2e3f"},{"index":196,"color":"#ff1122"}],"overrides":[],"indices":[1,196]}"##,
    )
    .map_err(|err| err.to_string())?;
    let reset_resp: PaletteResolveResponse =
        serde_json::from_str(&reset).map_err(|err| err.to_string())?;
    assert_eq!(reset_resp.override_count, 0);
    assert!(reset_resp.resolved.iter().all(|entry| !entry.is_override));

    // Render and decode LCVT v2 snapshot, then verify tampered CRC-32 fails cleanly.
    let raw = vt_render_snapshot(
        r#"{"cols":16,"rows":2,"ansiText":"\u001b[1;31mhello\u001b[0m\r\nworld","themeConfig":"palette = 1=#fe1234\n"}"#,
    )
    .map_err(|err| err.to_string())?;
    assert_eq!(&raw[..4], b"LCVT");
    let decoded_json = vt_decode_snapshot_json(&raw).map_err(|err| err.to_string())?;
    let decoded: VtDecodedSnapshotResponse =
        serde_json::from_str(&decoded_json).map_err(|err| err.to_string())?;
    assert_eq!(decoded.cols, 16);
    assert_eq!(decoded.rows, 2);
    assert_eq!(decoded.viewport_lines, vec!["hello", "world"]);
    assert_eq!(decoded.palette_overrides.len(), 1);
    assert_eq!(decoded.palette_overrides[0].color.hex, "#fe1234");

    let mut tampered = raw;
    tampered[8] ^= 0x01;
    assert!(vt_decode_snapshot_json(&tampered).is_err());
    Ok(())
}

#[test]
fn bug26_host_swap_and_theme_plan_preserve_body_and_isolate_panes() -> Result<(), String> {
    let swap_json = host_swap_chunks_json(
        r#"{"fromHost":"lclo.test","toHost":"127.0.0.1","chunks":["POST /api HTTP/1.1\r\nHo","st: LCLO.TEST:4443\r\n\r\n{\"prompt\":\"Host: lclo.test\"}","Host: lclo.test"]}"#,
    )
    .map_err(|err| err.to_string())?;
    let swap: HostSwapChunksResponse =
        serde_json::from_str(&swap_json).map_err(|err| err.to_string())?;
    assert_eq!(swap.phase, HttpProxyPhase::BodyPassthrough);
    assert_eq!(swap.transitioned_at_chunk, Some(1));
    assert!(swap.output.contains("Host: 127.0.0.1:4443\r\n"));
    assert!(swap.output.contains(r#"{"prompt":"Host: lclo.test"}"#));
    assert!(swap.output.ends_with("Host: lclo.test"));

    let plan_json = theme_plan_json(
        r#"{
            "operations": [
                { "paneId": "pane/left:1", "themeConfig": "background = #111111", "fontSize": 14.0, "appearance": "dark" },
                { "paneId": "pane/right:2", "themeConfig": "background = #111111", "fontSize": 14.0, "appearance": "dark" },
                { "paneId": "pane/left:1", "themeConfig": "background = #111111", "fontSize": 14.0, "appearance": "dark" },
                { "paneId": "pane/left:1", "themeConfig": "background = #ffffff", "fontSize": 16.0, "appearance": "light" }
            ]
        }"#,
    )
    .map_err(|err| err.to_string())?;
    let plan: ThemePlanResponse =
        serde_json::from_str(&plan_json).map_err(|err| err.to_string())?;
    assert_eq!(plan.pane_count, 2);
    assert_ne!(plan.results[0].filename, plan.results[1].filename);
    assert_eq!(plan.results[2].changed, Some(false));
    assert_eq!(plan.results[3].changed, Some(true));
    assert_eq!(plan.results[3].obsolete_filename, plan.results[0].filename);
    Ok(())
}
