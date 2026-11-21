//! Pure-Rust host-testable bridge modules backing the `#[wasm_bindgen]` surface.
//!
//! All logic lives in standard Rust functions and structs using `&str`, `String`, `&[u8]`,
//! `Vec<u8>`, and `Result<T, String>` so host tests run natively on Windows/Linux/macOS without
//! triggering `wasm-bindgen` `JsValue` host-panic stubs.
//!
//! # Examples
//!
//! ```
//! use t3_wasm_ffi::bridge::{
//!     ClientCoreBridge, EdgeWorkerBridge, TerminalVtBridge, decode_t3vt_summary_json,
//!     encode_t3vt_from_ansi, palette_search_json, sanitize_bracketed_paste_str,
//! };
//!
//! let clean = sanitize_bracketed_paste_str("ok\x1b[201~evil\x00!");
//! assert_eq!(clean, "okevil!");
//!
//! let t3vt = encode_t3vt_from_ansi(40, 10, "\x1b[32mhello\x1b[0m")?;
//! let summary: serde_json::Value = serde_json::from_str(&decode_t3vt_summary_json(&t3vt)?)?;
//! assert_eq!(summary["lines"][0], "hello");
//!
//! let mut core = ClientCoreBridge::new(1_000);
//! assert_eq!(core.next_backoff_ms(0), 1_500);
//!
//! let mut vt = TerminalVtBridge::new(20, 4)?;
//! vt.feed("hi");
//! let exported = vt.export_t3vt();
//! assert!(exported.starts_with(b"T3VT"));
//!
//! let search: serde_json::Value = serde_json::from_str(&palette_search_json(
//!     r#"{"query":"auth","titles":["Fix oauth","Share auth"]}"#,
//! )?)?;
//! assert_eq!(search["matches"][0]["itemIndex"], 1);
//!
//! let mut worker = EdgeWorkerBridge::new();
//! let health: serde_json::Value =
//!     serde_json::from_str(&worker.handle_request_json(r#"{"path":"/health"}"#)?)?;
//! assert_eq!(health["status"], 200);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

pub use crate::bridge_client::ClientCoreBridge;
pub use crate::bridge_hook_actor::HookInboxDurableObjectBridge;
pub use crate::bridge_shared::{palette_search_json, split_utf16_frame_parts_json};
pub use crate::bridge_surfaces::{
    decorate_diff_runs_json, evaluate_rpc_scopes_json, translate_i18n_json,
};
pub use crate::bridge_terminal::{
    TerminalVtBridge, decode_t3vt_summary_json, encode_t3vt_from_ansi, sanitize_bracketed_paste_str,
};
pub use crate::bridge_wave5::{
    active_brand_json, allows_preview_tunnel_json, highlight_hdl_json, link_batch_plan_json,
    normalize_nfd_filename, resolve_linked_thread_open_json, strip_osc4_queries,
};
pub use crate::bridge_web_ui::{WebAppBridge, emit_web_app_bundle_json};
pub use crate::bridge_worker::EdgeWorkerBridge;

const MAX_SAFE_JS_INTEGER: u64 = (1_u64 << 53) - 1;

/// Validates and converts a JavaScript `number` (`f64`) millisecond timestamp into `u64`.
///
/// Rejects `NaN`, infinities, negative numbers, and integers above `Number.MAX_SAFE_INTEGER`
/// (`2^53 - 1`) without truncation casts.
///
/// # Examples
///
/// ```
/// use t3_wasm_ffi::bridge::{js_ms_to_u64, u64_to_f64_exact};
///
/// assert_eq!(js_ms_to_u64(0.0), Ok(0));
/// assert_eq!(js_ms_to_u64(1_732_118_400_123.0), Ok(1_732_118_400_123));
/// assert_eq!(u64_to_f64_exact(1_732_118_400_123), 1_732_118_400_123.0);
/// assert!(js_ms_to_u64(-1.0).is_err());
/// assert!(js_ms_to_u64(f64::NAN).is_err());
/// ```
pub fn js_ms_to_u64(ms: f64) -> Result<u64, String> {
    if !ms.is_finite() || ms < 0.0 || ms > u64_to_f64_exact(MAX_SAFE_JS_INTEGER) {
        return Err(format!("invalid millisecond timestamp: {ms}"));
    }
    let truncated = ms.trunc();
    let mut lo = 0_u64;
    let mut hi = MAX_SAFE_JS_INTEGER;
    while lo < hi {
        let mid = lo + (hi - lo).div_ceil(2);
        if u64_to_f64_exact(mid) <= truncated {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    Ok(lo)
}

/// Converts a `u64` value (`<= 2^53 - 1` for exact precision) to `f64` without lossy `as` casts.
pub fn u64_to_f64_exact(value: u64) -> f64 {
    let bytes = value.to_le_bytes();
    let lo = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let hi = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    f64::from(hi) * 4_294_967_296.0 + f64::from(lo)
}
