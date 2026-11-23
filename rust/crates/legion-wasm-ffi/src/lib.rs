//! Quarantined `WebAssembly` `FFI` boundary crate for `LegionCode` browser, Node, and
//! Cloudflare Worker surfaces.
//!
//! This crate is the sole `wasm-bindgen` and `js-sys` bridge for web/edge/JS builds,
//! allowing all underlying core crates (`legion-contracts`, `legion-shared`, `legion-client-core`,
//! `legion-edge`, `legion-surfaces`, `legion-terminal-vt`) to remain pure `#![forbid(unsafe_code)]` Rust.
//! All logic lives in the host-testable [`bridge`] module with zero manual `unsafe` blocks.

#![deny(unsafe_op_in_unsafe_fn)]

pub mod bridge;
mod bridge_client;
mod bridge_client_dto;
mod bridge_client_family;
mod bridge_hook_actor;
mod bridge_hook_actor_dto;
mod bridge_hook_actor_fetch;
mod bridge_shared;
mod bridge_surfaces;
mod bridge_terminal;
mod bridge_wave5;
mod bridge_worker;
mod bridge_worker_bindings;
mod hdl_highlight;
mod hdl_keywords;
mod wasm_edge;

pub use wasm_edge::{WasmEdgeWorker, WasmHookInboxObject};

use wasm_bindgen::prelude::{JsValue, wasm_bindgen};

/// Returns the `ABI` version implemented by this quarantined `WASM` `FFI` crate.
///
/// # Examples
///
/// ```
/// assert_eq!(legion_wasm_ffi::abi_version(), 1);
/// ```
pub const fn abi_version() -> u32 {
    1
}

/// Exports the `WASM` `ABI` version (`1`) to JavaScript callers.
///
/// # Examples
///
/// ```
/// assert_eq!(legion_wasm_ffi::legion_wasm_abi_version(), 1);
/// ```
#[wasm_bindgen]
pub fn legion_wasm_abi_version() -> u32 {
    abi_version()
}

/// Splits and coalesces syntax diff runs against intra-line UTF-16 word-diff ranges (`BUG-22`).
#[wasm_bindgen]
pub fn decorate_diff_runs_json(input_json: &str) -> Result<String, JsValue> {
    bridge::decorate_diff_runs_json(input_json).map_err(|err| JsValue::from_str(&err))
}

/// Splits a UTF-16 code-unit array or UTF-8 string into WebSocket `?frameParts` chunks (`BUG-21`).
#[wasm_bindgen]
pub fn split_utf16_frame_parts_json(input_json: &str) -> Result<String, JsValue> {
    bridge::split_utf16_frame_parts_json(input_json).map_err(|err| JsValue::from_str(&err))
}

/// Runs `SoA` fuzzy command-palette search over `titles` or multi-field `entries`.
#[wasm_bindgen]
pub fn palette_search_json(input_json: &str) -> Result<String, JsValue> {
    bridge::palette_search_json(input_json).map_err(|err| JsValue::from_str(&err))
}

/// Resolves locale settings (`BUG-27`) and localized surface navigation/Live Activity (`BUG-24`).
#[wasm_bindgen]
pub fn translate_i18n_json(input_json: &str) -> Result<String, JsValue> {
    bridge::translate_i18n_json(input_json).map_err(|err| JsValue::from_str(&err))
}

/// Evaluates fail-closed multi-scope `RPC` authorization requirements (`BUG-17`).
#[wasm_bindgen]
pub fn evaluate_rpc_scopes_json(input_json: &str) -> Result<String, JsValue> {
    bridge::evaluate_rpc_scopes_json(input_json).map_err(|err| JsValue::from_str(&err))
}

/// Sanitizes untrusted clipboard text for safe bracketed-paste insertion.
///
/// # Examples
///
/// ```
/// let cleaned = legion_wasm_ffi::sanitize_bracketed_paste_str("hi\x1b[201~evil\x00!");
/// assert_eq!(cleaned, "hievil!");
/// ```
#[wasm_bindgen]
pub fn sanitize_bracketed_paste_str(raw: &str) -> String {
    bridge::sanitize_bracketed_paste_str(raw)
}

/// Renders ANSI/VT text on a `cols x rows` terminal grid and returns `"LCVT"` v2 snapshot bytes.
#[wasm_bindgen]
pub fn encode_lcvt_from_ansi(cols: u16, rows: u16, ansi_input: &str) -> Result<Vec<u8>, JsValue> {
    bridge::encode_lcvt_from_ansi(cols, rows, ansi_input).map_err(|err| JsValue::from_str(&err))
}

/// Decodes `"LCVT"` v1/v2 binary snapshot bytes and returns a JSON summary.
#[wasm_bindgen]
pub fn decode_lcvt_summary_json(bytes: &[u8]) -> Result<String, JsValue> {
    bridge::decode_lcvt_summary_json(bytes).map_err(|err| JsValue::from_str(&err))
}

/// Normalizes macOS NFD filenames and returns clean NFC display names.
#[wasm_bindgen]
pub fn normalize_nfd_filename(raw: &str) -> String {
    bridge::normalize_nfd_filename(raw)
}

/// Strips standalone `OSC 4` palette queries and dynamic color queries from terminal history.
#[wasm_bindgen]
pub fn strip_osc4_queries(raw: &str) -> String {
    bridge::strip_osc4_queries(raw)
}

/// Evaluates loopback preview tunnel policy and settings allowances (`{"allowed": bool}`).
#[wasm_bindgen]
pub fn allows_preview_tunnel_json(input_json: &str) -> Result<String, JsValue> {
    bridge::allows_preview_tunnel_json(input_json).map_err(|err| JsValue::from_str(&err))
}

/// Serializes the active brand configuration (`ACTIVE_BRAND`) as `JSON`.
#[wasm_bindgen]
pub fn active_brand_json() -> Result<String, JsValue> {
    bridge::active_brand_json().map_err(|err| JsValue::from_str(&err))
}

/// Stateful client-runtime state machine exported to JavaScript/WASM callers.
#[wasm_bindgen]
pub struct WasmClientCore {
    inner: bridge::ClientCoreBridge,
}

#[wasm_bindgen]
impl WasmClientCore {
    /// Creates a new [`WasmClientCore`] at explicit JavaScript timestamp `now_ms`.
    #[wasm_bindgen(constructor)]
    pub fn new(now_ms: f64) -> Result<Self, JsValue> {
        let ts = bridge::js_ms_to_u64(now_ms).map_err(|err| JsValue::from_str(&err))?;
        Ok(Self {
            inner: bridge::ClientCoreBridge::new(ts),
        })
    }

    /// Enqueues environment shell updates and steps steady-shell release coordination (`BUG-21`).
    #[wasm_bindgen]
    pub fn enqueue_steady_shell(&mut self, json: &str) -> Result<String, JsValue> {
        self.inner
            .enqueue_steady_shell(json)
            .map_err(|err| JsValue::from_str(&err))
    }

    /// Polls steady-shell release readiness at `now_ms`.
    #[wasm_bindgen]
    pub fn poll_steady_ready(&mut self, now_ms: f64) -> Result<String, JsValue> {
        let ts = bridge::js_ms_to_u64(now_ms).map_err(|err| JsValue::from_str(&err))?;
        self.inner
            .poll_steady_ready(ts)
            .map_err(|err| JsValue::from_str(&err))
    }

    /// Updates or queries the generation-keyed thread-family cache.
    #[wasm_bindgen]
    pub fn touch_family(&mut self, json: &str) -> Result<String, JsValue> {
        self.inner
            .touch_family(json)
            .map_err(|err| JsValue::from_str(&err))
    }

    /// Records a transient disconnect at `now_ms` and returns the deterministic backoff decision.
    #[wasm_bindgen]
    pub fn record_disconnect(&mut self, now_ms: f64) -> Result<String, JsValue> {
        let ts = bridge::js_ms_to_u64(now_ms).map_err(|err| JsValue::from_str(&err))?;
        self.inner
            .record_disconnect(ts)
            .map_err(|err| JsValue::from_str(&err))
    }

    /// Computes the deterministic midpoint-jittered reconnect delay in ms for 0-based `attempt`.
    #[wasm_bindgen]
    pub fn next_backoff_ms(&self, attempt: u32) -> u32 {
        self.inner.next_backoff_ms(attempt)
    }

    /// Advances the connected-lease silent-socket probe state machine with one JSON command.
    #[wasm_bindgen]
    pub fn probe_step_json(&mut self, json: &str) -> Result<String, JsValue> {
        self.inner
            .probe_step_json(json)
            .map_err(|err| JsValue::from_str(&err))
    }

    /// Resolves linked-thread opening strategy (`direct` vs. `relay`).
    #[wasm_bindgen]
    pub fn linked_thread_open_json(&self, input_json: &str) -> Result<String, JsValue> {
        bridge::resolve_linked_thread_open_json(input_json).map_err(|err| JsValue::from_str(&err))
    }

    /// Plans bounded-concurrency P2P batch linking candidates and next batch.
    #[wasm_bindgen]
    pub fn link_batch_plan_json(&self, input_json: &str) -> Result<String, JsValue> {
        bridge::link_batch_plan_json(input_json).map_err(|err| JsValue::from_str(&err))
    }
}

/// Stateful streaming VT terminal and sparse ANSI palette engine exported to JavaScript callers.
#[wasm_bindgen]
pub struct WasmTerminalVt {
    inner: bridge::TerminalVtBridge,
}

#[wasm_bindgen]
impl WasmTerminalVt {
    /// Creates a bounded terminal grid validated to `1..=1024 x 1..=512`.
    #[wasm_bindgen(constructor)]
    pub fn new(cols: u16, rows: u16) -> Result<Self, JsValue> {
        let inner =
            bridge::TerminalVtBridge::new(cols, rows).map_err(|err| JsValue::from_str(&err))?;
        Ok(Self { inner })
    }

    /// Feeds UTF-8 / ANSI / OSC text incrementally into the terminal parser.
    #[wasm_bindgen]
    pub fn feed(&mut self, text: &str) {
        self.inner.feed(text);
    }

    /// Feeds raw PTY bytes incrementally into the terminal parser.
    #[wasm_bindgen]
    pub fn feed_bytes(&mut self, bytes: &[u8]) {
        self.inner.feed_bytes(bytes);
    }

    /// Applies Ghostty theme configuration / sparse ANSI palette overrides (`BUG-25`).
    #[wasm_bindgen]
    pub fn apply_theme_json(&mut self, json: &str) -> Result<String, JsValue> {
        self.inner
            .apply_theme_json(json)
            .map_err(|err| JsValue::from_str(&err))
    }

    /// Exports the current viewport as a CRC-32 checked `"LCVT"` v2 binary snapshot.
    #[wasm_bindgen]
    pub fn export_lcvt(&self) -> Vec<u8> {
        self.inner.export_lcvt()
    }

    /// Serializes a JSON summary of the current terminal viewport and palette state.
    #[wasm_bindgen]
    pub fn snapshot_summary_json(&self) -> Result<String, JsValue> {
        self.inner
            .snapshot_summary_json()
            .map_err(|err| JsValue::from_str(&err))
    }
}

#[cfg(test)]
mod tests {
    use super::{abi_version, legion_wasm_abi_version, sanitize_bracketed_paste_str};

    #[test]
    fn reports_expected_wasm_abi_version_and_paste_sanitization() {
        assert_eq!(abi_version(), 1);
        assert_eq!(legion_wasm_abi_version(), 1);
        assert_eq!(
            sanitize_bracketed_paste_str("ok\t\n\x1b[201~bad\x1b]0;title\x07!"),
            "ok\t\nbad!"
        );
    }
}
