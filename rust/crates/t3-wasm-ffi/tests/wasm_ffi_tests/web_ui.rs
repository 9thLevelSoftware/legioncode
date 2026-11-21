//! Tests for [`t3_wasm_ffi::bridge::WebAppBridge`] and `emit_web_app_bundle_json`.

use serde_json::Value;
use t3_wasm_ffi::bridge::{WebAppBridge, emit_web_app_bundle_json};

fn parse_json(raw: &str) -> Result<Value, String> {
    serde_json::from_str(raw).map_err(|err| format!("failed to parse JSON: {err} ({raw})"))
}

#[test]
fn web_app_bridge_dispatches_mvu_actions_renders_xss_safe_html_and_snapshots() -> Result<(), String>
{
    let mut app = WebAppBridge::new("sv-SE", 1_000);
    let initial_snap = parse_json(&app.state_snapshot_json()?)?;
    assert_eq!(initial_snap["locale"]["catalogLocale"], "sv");
    assert_eq!(initial_snap["locale"]["formattingLocale"], "sv-SE");

    // XSS payload in composer draft must be HTML-escaped in rendered HTML.
    let draft_out = parse_json(
        &app.dispatch_json(r#"{"type":"updateComposerDraft","text":"<script>alert(1)</script>"}"#)?,
    )?;
    let html = draft_out["html"].as_str().ok_or("missing html")?;
    assert!(!html.contains("<script>alert(1)</script>"));
    assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));

    // Switch tab to terminal, feed VT output, and verify rendered terminal grid + snapshot.
    let _ = app.dispatch_json(r#"{"type":"setRightTab","tab":"terminal"}"#)?;
    let term_out = parse_json(
        &app.dispatch_json(r#"{"type":"terminalFeed","text":"\u001b[32mready\u001b[0m"}"#)?,
    )?;
    let term_html = term_out["html"].as_str().ok_or("missing terminal html")?;
    assert!(term_html.contains(r#"data-testid="terminal-grid""#));
    assert!(term_html.contains("ready"));
    assert_eq!(app.render_html(), term_html);

    // Command palette search & pairing action emit typed WebHostEffect entries.
    let pair_input =
        parse_json(&app.dispatch_json(r#"{"type":"setPairingInput","value":"tok-secret"}"#)?)?;
    assert_eq!(pair_input["state"]["auth"]["pairingInput"], "tok-secret");
    let submit = parse_json(&app.dispatch_json(r#"{"type":"submitPairing"}"#)?)?;
    let effects = submit["effects"].as_array().ok_or("missing effects")?;
    assert_eq!(effects.len(), 1);
    assert_eq!(effects[0]["type"], "httpFetch");
    assert_eq!(effects[0]["url"], "/api/auth/pair");

    assert!(app.dispatch_json(r#"{"type":"unknownAction"}"#).is_err());
    Ok(())
}

#[test]
fn emit_web_app_bundle_json_returns_deterministic_static_assets() -> Result<(), String> {
    let bundle = parse_json(&emit_web_app_bundle_json()?)?;
    let index_html = bundle["indexHtml"].as_str().ok_or("missing indexHtml")?;
    let styles_css = bundle["stylesCss"].as_str().ok_or("missing stylesCss")?;
    let app_mjs = bundle["appMjs"].as_str().ok_or("missing appMjs")?;

    assert!(index_html.contains("styles.css"));
    assert!(index_html.contains("app.mjs"));
    assert!(styles_css.contains(".t3-shell"));
    assert!(app_mjs.contains("WasmWebApp"));
    assert!(app_mjs.contains("./t3_wasm_ffi.js"));
    assert!(app_mjs.contains("dispatch_json"));
    Ok(())
}
