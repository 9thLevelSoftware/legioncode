//! Tests for [`legion_wasm_ffi::bridge::TerminalVtBridge`] and `LCVT`/paste helpers.

use legion_wasm_ffi::bridge::{
    TerminalVtBridge, decode_lcvt_summary_json, encode_lcvt_from_ansi, sanitize_bracketed_paste_str,
};
use serde_json::Value;

fn parse_json(raw: &str) -> Result<Value, String> {
    serde_json::from_str(raw).map_err(|err| format!("failed to parse JSON: {err} ({raw})"))
}

#[test]
fn bracketed_paste_sanitizer_neutralizes_escape_and_osc_injections() {
    let raw = "echo ok\t\r\n\x1b[201~rm -rf /\x1b[000200~\x1b]0;pwn\x07\x00!";
    assert_eq!(sanitize_bracketed_paste_str(raw), "echo ok\t\nrm -rf /!");
}

#[test]
fn encode_and_decode_lcvt_round_trips_and_rejects_corruption() -> Result<(), String> {
    let bytes = encode_lcvt_from_ansi(40, 8, "\x1b[32mhello\x1b[0m\r\nworld")?;
    assert!(bytes.starts_with(b"LCVT"));

    let summary = parse_json(&decode_lcvt_summary_json(&bytes)?)?;
    assert_eq!(summary["cols"], 40);
    assert_eq!(summary["rows"], 8);
    assert_eq!(summary["cursorX"], 5);
    assert_eq!(summary["cursorY"], 1);
    assert_eq!(summary["lines"][0], "hello");
    assert_eq!(summary["lines"][1], "world");
    assert_eq!(summary["nonEmptyCellCount"], 10);

    // Flipping a bit must fail CRC-32 validation.
    let mut corrupted = bytes;
    if let Some(last) = corrupted.last_mut() {
        *last ^= 0x01;
    }
    assert!(decode_lcvt_summary_json(&corrupted).is_err());
    assert!(encode_lcvt_from_ansi(0, 24, "x").is_err());
    Ok(())
}

#[test]
fn terminal_vt_bridge_handles_streaming_osc_and_sparse_theme_reset() -> Result<(), String> {
    let mut vt = TerminalVtBridge::new(30, 6)?;
    vt.feed("\x1b]2;Build");
    vt.feed(" Shell\x07\x1b[1;31mok\x1b[0m");

    let themed = parse_json(&vt.apply_theme_json(
        r##"{
            "foreground": "#e6edf3",
            "background": "#0d1117",
            "palette": [
                {"index": 1, "color": "#ff7b72"},
                {"index": 196, "color": "#fa0a0a"}
            ]
        }"##,
    )?)?;
    assert_eq!(themed["overrideCount"], 2);
    assert_eq!(themed["overrides"][0]["index"], 1);
    assert_eq!(themed["overrides"][1]["index"], 196);
    assert_eq!(themed["foreground"], "#e6edf3");

    let snap = parse_json(&vt.snapshot_summary_json()?)?;
    assert_eq!(snap["title"], "Build Shell");
    assert_eq!(snap["lines"][0], "ok");
    assert_eq!(snap["paletteOverrideCount"], 2);

    // BUG-25: empty theme resets all 256 palette slots without leaving stale overrides.
    let cleared = parse_json(&vt.apply_theme_json("{}")?)?;
    assert_eq!(cleared["overrideCount"], 0);
    let snap_after = parse_json(&vt.snapshot_summary_json()?)?;
    assert_eq!(snap_after["paletteOverrideCount"], 0);

    let exported = vt.export_lcvt();
    let decoded = parse_json(&decode_lcvt_summary_json(&exported)?)?;
    assert_eq!(decoded["lines"][0], "ok");
    assert_eq!(decoded["paletteOverrideCount"], 0);
    Ok(())
}
