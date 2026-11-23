//! Desktop `appleSpeech` IPC method handlers wired to `legion_speech::apple_speech` (`#34027`).

use legion_contracts::ipc::{
    DesktopAppleSpeechErrorCode, DesktopAppleSpeechResult, DesktopAppleSpeechTranscribeInput,
};
use legion_speech::apple_speech::{
    AppleSpeechAssetStatus, AppleSpeechCapabilities, AppleSpeechSession, AppleSpeechState,
    map_helper_exit_code,
};
use serde_json::{Value, json};

use crate::ipc_handlers::{decode_params, encode_result, ipc_error};
use crate::ipc_types::DesktopIpcErrorPayload;

/// Executes macOS Apple Speech helper steps against a backend (real process or test stub).
pub trait AppleSpeechHelper {
    /// Query supported locale strings (e.g. `["en-US", "fr-FR"]`).
    fn supported_locales(&mut self) -> Vec<String>;

    /// Download/prepare the local speech model for `locale`.
    fn prepare(
        &mut self,
        locale: &str,
    ) -> Result<AppleSpeechAssetStatus, DesktopAppleSpeechErrorCode>;

    /// Transcribe 16-bit WAV audio generated from 16 kHz mono Float32 PCM (`#34027`).
    fn transcribe(
        &mut self,
        locale: &str,
        wav_bytes: &[u8],
    ) -> Result<String, DesktopAppleSpeechErrorCode>;
}

/// Default unavailable helper used on non-macOS hosts or when helper binary is absent (`#34027`).
#[derive(Debug, Default, Clone)]
pub struct UnavailableAppleSpeechHelper;

impl AppleSpeechHelper for UnavailableAppleSpeechHelper {
    fn supported_locales(&mut self) -> Vec<String> {
        Vec::new()
    }

    fn prepare(
        &mut self,
        _locale: &str,
    ) -> Result<AppleSpeechAssetStatus, DesktopAppleSpeechErrorCode> {
        Err(DesktopAppleSpeechErrorCode::Unavailable)
    }

    fn transcribe(
        &mut self,
        _locale: &str,
        _wav_bytes: &[u8],
    ) -> Result<String, DesktopAppleSpeechErrorCode> {
        Err(DesktopAppleSpeechErrorCode::Unavailable)
    }
}

/// Helper adapter converting exit codes + JSON output matching `Native AppleSpeech.swift` (`#34027`).
pub fn parse_helper_json_value(
    stdout: &str,
    exit_code: i32,
) -> Result<String, DesktopAppleSpeechErrorCode> {
    if exit_code != 0 {
        return Err(map_helper_exit_code(exit_code));
    }
    let trimmed = stdout.trim();
    if let Ok(val) = serde_json::from_str::<Value>(trimmed)
        && let Some(text) = val.get("value").and_then(Value::as_str)
    {
        return Ok(text.to_owned());
    }
    Ok(trimmed.to_owned())
}

/// Stateful service pairing [`AppleSpeechSession`] with helper execution (`#34027`).
#[derive(Debug, Clone)]
pub struct AppleSpeechIpcService {
    session: AppleSpeechSession,
}

impl Default for AppleSpeechIpcService {
    fn default() -> Self {
        Self::new(AppleSpeechCapabilities::default())
    }
}

impl AppleSpeechIpcService {
    /// Create a new [`AppleSpeechIpcService`] initialized with `capabilities`.
    pub fn new(capabilities: AppleSpeechCapabilities) -> Self {
        Self {
            session: AppleSpeechSession::new(capabilities),
        }
    }

    /// Inspect the current underlying [`AppleSpeechSession`].
    pub const fn session(&self) -> &AppleSpeechSession {
        &self.session
    }

    /// Update advertised Apple Speech capabilities (for instance after querying helper locales).
    pub fn update_capabilities(&mut self, capabilities: AppleSpeechCapabilities) {
        self.session.update_capabilities(capabilities);
    }

    /// Handle `desktop.appleSpeech.available` / `supportedLocales` (`#34027`).
    pub fn available(&self) -> Value {
        let locales = self.session.supported_locales();
        json!({
            "available": self.session.capabilities().available && !locales.is_empty(),
            "supportedLocales": locales,
        })
    }

    /// Handle `desktop.appleSpeech.supportedLocales` returning just the locale array (`#34027`).
    pub fn supported_locales(&self) -> Vec<String> {
        self.session.supported_locales().to_vec()
    }

    /// Handle `desktop.appleSpeech.prepare` (`#34027`).
    pub fn prepare<H: AppleSpeechHelper>(
        &mut self,
        locale: &str,
        helper: &mut H,
    ) -> DesktopAppleSpeechResult {
        if self.session.capabilities().supported_locales.is_empty() {
            let discovered = helper.supported_locales();
            if !discovered.is_empty() {
                self.session
                    .update_capabilities(AppleSpeechCapabilities::new(true, discovered));
            }
        }
        let resolved_locale = match self.session.begin_prepare(locale) {
            Ok(loc) => loc,
            Err(err_res) => return err_res,
        };
        match helper.prepare(&resolved_locale) {
            Ok(status) => self.session.finish_prepare(status),
            Err(code) => {
                self.session.reset();
                DesktopAppleSpeechResult::err(code)
            }
        }
    }

    /// Handle `desktop.appleSpeech.transcribe` (`#34027`).
    pub fn transcribe<H: AppleSpeechHelper>(
        &mut self,
        input: &DesktopAppleSpeechTranscribeInput,
        helper: &mut H,
    ) -> DesktopAppleSpeechResult {
        if self.session.capabilities().supported_locales.is_empty() {
            let discovered = helper.supported_locales();
            if !discovered.is_empty() {
                self.session
                    .update_capabilities(AppleSpeechCapabilities::new(true, discovered));
            }
        }
        let wav_bytes = match self.session.begin_transcribe(input) {
            Ok(wav) => wav,
            Err(err_res) => return err_res,
        };
        let resolved_locale = match self.session.state() {
            AppleSpeechState::Transcribing { resolved_locale } => resolved_locale.clone(),
            _ => {
                return self
                    .session
                    .finish_transcribe(Err(DesktopAppleSpeechErrorCode::Failed));
            }
        };
        let helper_res = helper.transcribe(&resolved_locale, &wav_bytes);
        self.session
            .finish_transcribe(helper_res.as_deref().map_err(|&c| c))
    }
}

pub(crate) fn handle_apple_speech_available(
    svc: &AppleSpeechIpcService,
) -> Result<Value, DesktopIpcErrorPayload> {
    encode_result(svc.available())
}

pub(crate) fn handle_apple_speech_supported_locales(
    svc: &AppleSpeechIpcService,
) -> Result<Value, DesktopIpcErrorPayload> {
    encode_result(svc.supported_locales())
}

pub(crate) fn handle_apple_speech_prepare<H: AppleSpeechHelper>(
    svc: &mut AppleSpeechIpcService,
    params: &Value,
    helper: &mut H,
) -> Result<Value, DesktopIpcErrorPayload> {
    let locale = if let Some(raw) = params.as_str() {
        raw.to_owned()
    } else if let Some(raw) = params.get("locale").and_then(Value::as_str) {
        raw.to_owned()
    } else {
        return Err(ipc_error(
            "InvalidParams",
            "locale string or { locale } object is required for desktop.appleSpeech.prepare",
        ));
    };
    encode_result(svc.prepare(&locale, helper))
}

pub(crate) fn handle_apple_speech_transcribe<H: AppleSpeechHelper>(
    svc: &mut AppleSpeechIpcService,
    params: Value,
    helper: &mut H,
) -> Result<Value, DesktopIpcErrorPayload> {
    let input: DesktopAppleSpeechTranscribeInput = decode_params(params)?;
    encode_result(svc.transcribe(&input, helper))
}
