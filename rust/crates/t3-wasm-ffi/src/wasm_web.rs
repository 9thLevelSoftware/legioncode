//! `#[wasm_bindgen]` web MVU workspace engine and static bundle exports.

use wasm_bindgen::prelude::{JsValue, wasm_bindgen};

use crate::bridge;

/// Stateful Model-View-Update web workspace engine exported to JavaScript callers.
#[wasm_bindgen]
pub struct WasmWebApp {
    inner: bridge::WebAppBridge,
}

#[wasm_bindgen]
impl WasmWebApp {
    /// Initializes a new [`WasmWebApp`] from `locale_hint` and JavaScript timestamp `now_ms`.
    #[wasm_bindgen(constructor)]
    pub fn new(locale_hint: &str, now_ms: f64) -> Result<Self, JsValue> {
        let ts = bridge::js_ms_to_u64(now_ms).map_err(|err| JsValue::from_str(&err))?;
        Ok(Self {
            inner: bridge::WebAppBridge::new(locale_hint, ts),
        })
    }

    /// Dispatches one JSON [`t3_web_ui::WebAppAction`] and returns
    /// `{ "effects": [...], "html": "...", "state": {...} }`.
    #[wasm_bindgen]
    pub fn dispatch_json(&mut self, action_json: &str) -> Result<String, JsValue> {
        self.inner
            .dispatch_json(action_json)
            .map_err(|err| JsValue::from_str(&err))
    }

    /// Renders the `XSS`-safe `HTML5` workspace view for the current state.
    #[wasm_bindgen]
    pub fn render_html(&self) -> String {
        self.inner.render_html()
    }

    /// Serializes the current [`t3_web_ui::WebAppStateSnapshot`] as JSON.
    #[wasm_bindgen]
    pub fn state_snapshot_json(&self) -> Result<String, JsValue> {
        self.inner
            .state_snapshot_json()
            .map_err(|err| JsValue::from_str(&err))
    }
}

/// Emits the deterministic static web bundle (`indexHtml`, `stylesCss`, `appMjs`) as JSON.
#[wasm_bindgen]
pub fn emit_web_app_bundle_json() -> Result<String, JsValue> {
    bridge::emit_web_app_bundle_json().map_err(|err| JsValue::from_str(&err))
}
