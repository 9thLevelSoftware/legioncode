//! Pure-Rust host-testable bridge for [`t3_web_ui`] backing `WasmWebApp`.

use t3_contracts::orchestration::StreamItem;
use t3_web_ui::{WebAppAction, WebAppEngine, emit_web_app_assets};

/// Stateful Model-View-Update web UI bridge wrapping [`WebAppEngine`].
#[derive(Debug, Clone)]
pub struct WebAppBridge {
    engine: WebAppEngine,
}

impl WebAppBridge {
    /// Initializes a new [`WebAppBridge`] from `locale_hint` and explicit `now_ms`.
    pub fn new(locale_hint: &str, now_ms: u64) -> Self {
        Self {
            engine: WebAppEngine::new(locale_hint, now_ms),
        }
    }

    /// Borrows the underlying [`WebAppEngine`].
    pub const fn engine(&self) -> &WebAppEngine {
        &self.engine
    }

    /// Dispatches one JSON [`t3_web_ui::WebAppAction`] and returns
    /// `{ "effects": [...], "html": "...", "state": {...} }`.
    pub fn dispatch_json(&mut self, action_json: &str) -> Result<String, String> {
        self.engine
            .dispatch_json(action_json)
            .map_err(|err| err.to_string())
    }

    /// Renders the `XSS`-safe `HTML5` workspace view for the current state.
    pub fn render_html(&self) -> String {
        self.engine.render_html()
    }

    /// Serializes the current [`t3_web_ui::WebAppStateSnapshot`] as JSON.
    pub fn state_snapshot_json(&self) -> Result<String, String> {
        self.engine
            .state_snapshot_json()
            .map_err(|err| err.to_string())
    }

    /// Applies one `StreamItem` JSON payload at `now_ms` and returns updated JSON snapshot.
    pub fn dispatch_stream_json(
        &mut self,
        stream_json: &str,
        now_ms: u64,
    ) -> Result<String, String> {
        let item: StreamItem = serde_json::from_str(stream_json).map_err(|err| err.to_string())?;
        let action_json = serde_json::to_string(&WebAppAction::ReceiveStreamItem {
            thread_id: None,
            item,
            now_ms: Some(now_ms),
        })
        .map_err(|err| err.to_string())?;
        self.dispatch_json(&action_json)
    }

    /// Tokenizes Verilog, `SystemVerilog`, or VHDL snippets (`t3_web_ui::hdl_highlight`).
    pub fn highlight_hdl_json(&self, lang_or_path: &str, source: &str) -> Result<String, String> {
        crate::bridge_wave5::highlight_hdl_json(lang_or_path, source)
    }
}

/// Emits the deterministic static web bundle (`indexHtml`, `stylesCss`, `appMjs`) as JSON.
pub fn emit_web_app_bundle_json() -> Result<String, String> {
    serde_json::to_string(&emit_web_app_assets())
        .map_err(|err| format!("failed to serialize web app bundle: {err}"))
}
