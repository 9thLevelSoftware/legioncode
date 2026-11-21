//! Pure-Rust host-testable bridge for [`t3_web_ui`] backing `WasmWebApp`.

use t3_web_ui::{WebAppEngine, emit_web_app_assets};

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
}

/// Emits the deterministic static web bundle (`indexHtml`, `stylesCss`, `appMjs`) as JSON.
pub fn emit_web_app_bundle_json() -> Result<String, String> {
    serde_json::to_string(&emit_web_app_assets())
        .map_err(|err| format!("failed to serialize web app bundle: {err}"))
}
