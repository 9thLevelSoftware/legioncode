//! Desktop IPC payloads mirrored from `packages/contracts/src/ipc.ts`.

use serde::{Deserialize, Serialize};

/// Error reason returned by macOS on-device Apple Speech recognition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DesktopAppleSpeechErrorCode {
    /// Apple Speech framework or model is unavailable on this host.
    Unavailable,
    /// Requested BCP-47 locale is not supported by Apple Speech.
    UnsupportedLocale,
    /// Speech analyzer failed to initialize or transcribe the audio buffer.
    Failed,
}

/// Discriminated wire result from `desktopBridge.appleSpeech.{prepare,transcribe}` (`ipc.ts`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DesktopAppleSpeechResult {
    /// Preparation resolved a locale or transcription produced final text.
    Ok {
        /// Always `true` on success.
        ok: bool,
        /// Resolved locale identifier or recognized transcript text.
        value: String,
    },
    /// Preparation or transcription failed cleanly.
    Err {
        /// Always `false` on failure.
        ok: bool,
        /// Failure reason code.
        code: DesktopAppleSpeechErrorCode,
    },
}

impl DesktopAppleSpeechResult {
    /// Constructs a successful Apple Speech IPC response.
    pub fn ok(value: impl Into<String>) -> Self {
        Self::Ok {
            ok: true,
            value: value.into(),
        }
    }

    /// Constructs a failed Apple Speech IPC response.
    pub const fn err(code: DesktopAppleSpeechErrorCode) -> Self {
        Self::Err { ok: false, code }
    }
}

/// Input payload for desktop Apple Speech transcription (`DesktopAppleSpeechTranscribeInputSchema`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopAppleSpeechTranscribeInput {
    /// BCP-47 locale code (up to 64 characters).
    pub locale: String,
    /// 16 kHz mono Float32 PCM bytes (up to 5 minutes).
    pub pcm: Vec<u8>,
}
