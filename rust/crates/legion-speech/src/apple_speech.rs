//! Native macOS/iOS Apple Speech dictation state machine and locale capability model (`285558eb10` #34027).
//!
//! Mirrors `apps/desktop/src/speech/AppleSpeech.ts` and `native/apple-speech/AppleSpeechEngine.swift`,
//! wired directly to `legion_contracts::ipc::{DesktopAppleSpeechErrorCode, DesktopAppleSpeechResult,
//! DesktopAppleSpeechTranscribeInput}`.

use legion_contracts::ipc::{
    DesktopAppleSpeechErrorCode, DesktopAppleSpeechResult, DesktopAppleSpeechTranscribeInput,
};
use serde::{Deserialize, Serialize};

use crate::types::SPEECH_SAMPLE_RATE_HZ;
use crate::wav::encode_wav_pcm16le;

/// Expected sample rate (`16 kHz`) for Apple Speech PCM uploads.
pub const APPLE_SPEECH_SAMPLE_RATE_HZ: u32 = SPEECH_SAMPLE_RATE_HZ;
/// Maximum input PCM byte length (`5 minutes` of `16 kHz` mono `f32le` = `19_200_000` bytes).
pub const MAX_APPLE_SPEECH_PCM_BYTES: usize = (APPLE_SPEECH_SAMPLE_RATE_HZ as usize) * 4 * 5 * 60;
/// Maximum BCP-47 locale identifier byte length (`DesktopAppleSpeechTranscribeInputSchema`).
pub const MAX_APPLE_SPEECH_LOCALE_LEN: usize = 64;
/// Timeout for model download/prepare (`10` minutes, `AppleSpeech.ts:24`).
pub const APPLE_SPEECH_PREPARE_TIMEOUT_MS: u64 = 10 * 60 * 1_000;
/// Timeout for file transcription (`5` minutes, `AppleSpeech.ts:25`).
pub const APPLE_SPEECH_TRANSCRIBE_TIMEOUT_MS: u64 = 5 * 60 * 1_000;

/// On-device `SpeechAnalyzer` asset status (`AssetInventory.status(forModules:)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AppleSpeechAssetStatus {
    Installed,
    Supported,
    Downloading,
    Unsupported,
    Unknown,
}

/// Host availability and supported BCP-47 locale catalog reported by `legion-apple-speech available`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppleSpeechCapabilities {
    pub available: bool,
    pub supported_locales: Vec<String>,
}

impl AppleSpeechCapabilities {
    /// Constructs capabilities from `available` and raw comma-separated or list locales.
    pub fn new(available: bool, locales: impl IntoIterator<Item = String>) -> Self {
        let supported_locales = if available {
            locales
                .into_iter()
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty() && s.len() <= MAX_APPLE_SPEECH_LOCALE_LEN)
                .collect()
        } else {
            Vec::new()
        };
        Self {
            available,
            supported_locales,
        }
    }

    /// Parses comma-separated locale output emitted by `legion-apple-speech available`.
    pub fn from_available_csv(csv: &str) -> Self {
        let locales: Vec<String> = csv
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        Self::new(!locales.is_empty(), locales)
    }

    /// Resolves an equivalent supported BCP-47 locale (`en` -> `en-US`, `pt_BR` -> `pt-BR`).
    pub fn resolve_locale(&self, requested: &str) -> Option<&str> {
        if !self.available {
            return None;
        }
        let normalized = requested.trim().replace('_', "-");
        if normalized.is_empty() || normalized.len() > MAX_APPLE_SPEECH_LOCALE_LEN {
            return None;
        }
        if let Some(exact) = self
            .supported_locales
            .iter()
            .find(|loc| loc.eq_ignore_ascii_case(&normalized))
        {
            return Some(exact.as_str());
        }
        let req_primary = normalized.split('-').next()?;
        self.supported_locales.iter().find_map(|loc| {
            let cand_primary = loc.split('-').next()?;
            cand_primary
                .eq_ignore_ascii_case(req_primary)
                .then_some(loc.as_str())
        })
    }
}

/// Lifecycle state of [`AppleSpeechSession`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "kebab-case")]
pub enum AppleSpeechState {
    Idle,
    Unavailable,
    Preparing {
        requested_locale: String,
        resolved_locale: String,
    },
    Ready {
        resolved_locale: String,
    },
    Transcribing {
        resolved_locale: String,
    },
    Faulted {
        code: DesktopAppleSpeechErrorCode,
    },
}

/// Helper JSON payload `{ "value": "..." }` written to stdout by `AppleSpeech.swift`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppleSpeechHelperOutput {
    pub value: String,
}

/// State machine driving on-device macOS/iOS Apple Speech dictation (`#34027`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppleSpeechSession {
    capabilities: AppleSpeechCapabilities,
    state: AppleSpeechState,
}

impl AppleSpeechSession {
    /// Creates a new session initialized from host `capabilities`.
    pub fn new(capabilities: AppleSpeechCapabilities) -> Self {
        let state = if capabilities.available {
            AppleSpeechState::Idle
        } else {
            AppleSpeechState::Unavailable
        };
        Self {
            capabilities,
            state,
        }
    }

    /// Returns the current [`AppleSpeechState`].
    pub const fn state(&self) -> &AppleSpeechState {
        &self.state
    }

    /// Returns the host [`AppleSpeechCapabilities`].
    pub const fn capabilities(&self) -> &AppleSpeechCapabilities {
        &self.capabilities
    }

    /// Lists supported BCP-47 locales when available, or empty when unavailable.
    pub fn supported_locales(&self) -> &[String] {
        if self.capabilities.available {
            &self.capabilities.supported_locales
        } else {
            &[]
        }
    }

    /// Updates capabilities dynamically (e.g. after querying the native helper).
    pub fn update_capabilities(&mut self, capabilities: AppleSpeechCapabilities) {
        self.capabilities = capabilities;
        if !self.capabilities.available {
            self.state = AppleSpeechState::Unavailable;
        } else if matches!(self.state, AppleSpeechState::Unavailable) {
            self.state = AppleSpeechState::Idle;
        }
    }

    /// Begins locale preparation (`Idle | Ready | Faulted -> Preparing`), validating locale support.
    pub fn begin_prepare(&mut self, locale: &str) -> Result<String, DesktopAppleSpeechResult> {
        if !self.capabilities.available {
            self.state = AppleSpeechState::Unavailable;
            return Err(DesktopAppleSpeechResult::err(
                DesktopAppleSpeechErrorCode::Unavailable,
            ));
        }
        let Some(resolved) = self.capabilities.resolve_locale(locale).map(str::to_owned) else {
            self.state = AppleSpeechState::Faulted {
                code: DesktopAppleSpeechErrorCode::UnsupportedLocale,
            };
            return Err(DesktopAppleSpeechResult::err(
                DesktopAppleSpeechErrorCode::UnsupportedLocale,
            ));
        };
        self.state = AppleSpeechState::Preparing {
            requested_locale: locale.trim().to_owned(),
            resolved_locale: resolved.clone(),
        };
        Ok(resolved)
    }

    /// Completes locale preparation from `AssetInventory` status, returning a typed IPC result.
    pub fn finish_prepare(&mut self, status: AppleSpeechAssetStatus) -> DesktopAppleSpeechResult {
        let AppleSpeechState::Preparing {
            resolved_locale, ..
        } = &self.state
        else {
            return self.fail_with(DesktopAppleSpeechErrorCode::Failed);
        };
        let resolved = resolved_locale.clone();
        match status {
            AppleSpeechAssetStatus::Installed
            | AppleSpeechAssetStatus::Supported
            | AppleSpeechAssetStatus::Downloading => {
                self.state = AppleSpeechState::Ready {
                    resolved_locale: resolved.clone(),
                };
                DesktopAppleSpeechResult::ok(resolved)
            }
            AppleSpeechAssetStatus::Unsupported => {
                self.fail_with(DesktopAppleSpeechErrorCode::UnsupportedLocale)
            }
            AppleSpeechAssetStatus::Unknown => self.fail_with(DesktopAppleSpeechErrorCode::Failed),
        }
    }

    /// Validates an IPC transcribe payload and encodes 16-bit WAV bytes for `AVAudioFile`.
    pub fn begin_transcribe(
        &mut self,
        input: &DesktopAppleSpeechTranscribeInput,
    ) -> Result<Vec<u8>, DesktopAppleSpeechResult> {
        if !self.capabilities.available {
            self.state = AppleSpeechState::Unavailable;
            return Err(DesktopAppleSpeechResult::err(
                DesktopAppleSpeechErrorCode::Unavailable,
            ));
        }
        let Some(resolved) = self
            .capabilities
            .resolve_locale(&input.locale)
            .map(str::to_owned)
        else {
            return Err(self.fail_with(DesktopAppleSpeechErrorCode::UnsupportedLocale));
        };
        let Ok(wav) = encode_apple_speech_wav(&input.pcm) else {
            return Err(self.fail_with(DesktopAppleSpeechErrorCode::Failed));
        };
        self.state = AppleSpeechState::Transcribing {
            resolved_locale: resolved,
        };
        Ok(wav)
    }

    /// Completes an in-flight transcription with text or error code, returning to `Ready` on success.
    pub fn finish_transcribe(
        &mut self,
        outcome: Result<&str, DesktopAppleSpeechErrorCode>,
    ) -> DesktopAppleSpeechResult {
        let AppleSpeechState::Transcribing { resolved_locale } = &self.state else {
            return self.fail_with(DesktopAppleSpeechErrorCode::Failed);
        };
        let locale = resolved_locale.clone();
        match outcome {
            Ok(transcript) => {
                self.state = AppleSpeechState::Ready {
                    resolved_locale: locale,
                };
                DesktopAppleSpeechResult::ok(transcript.trim())
            }
            Err(code) => self.fail_with(code),
        }
    }

    /// Applies an exit code + JSON stdout emitted by `legion-apple-speech` during prepare or transcribe.
    pub fn apply_helper_exit(
        &mut self,
        exit_code: i32,
        stdout_json: &str,
    ) -> DesktopAppleSpeechResult {
        if exit_code == 0 {
            let Ok(parsed) = serde_json::from_str::<AppleSpeechHelperOutput>(stdout_json) else {
                return self.fail_with(DesktopAppleSpeechErrorCode::Failed);
            };
            match &self.state {
                AppleSpeechState::Preparing { .. } => {
                    self.state = AppleSpeechState::Ready {
                        resolved_locale: parsed.value.clone(),
                    };
                    return DesktopAppleSpeechResult::ok(parsed.value);
                }
                AppleSpeechState::Transcribing { resolved_locale } => {
                    self.state = AppleSpeechState::Ready {
                        resolved_locale: resolved_locale.clone(),
                    };
                    return DesktopAppleSpeechResult::ok(parsed.value.trim());
                }
                _ => return DesktopAppleSpeechResult::ok(parsed.value),
            }
        }
        let code = map_helper_exit_code(exit_code);
        self.fail_with(code)
    }

    /// Resets session state back to `Idle` (or `Unavailable` when host speech is unavailable).
    pub fn reset(&mut self) {
        self.state = if self.capabilities.available {
            AppleSpeechState::Idle
        } else {
            AppleSpeechState::Unavailable
        };
    }

    fn fail_with(&mut self, code: DesktopAppleSpeechErrorCode) -> DesktopAppleSpeechResult {
        self.state = if code == DesktopAppleSpeechErrorCode::Unavailable {
            AppleSpeechState::Unavailable
        } else {
            AppleSpeechState::Faulted { code }
        };
        DesktopAppleSpeechResult::err(code)
    }
}

/// Maps `AppleSpeech.swift` process exit codes (`1`, `2`, `3`, `64`) to IPC error codes.
pub const fn map_helper_exit_code(exit_code: i32) -> DesktopAppleSpeechErrorCode {
    match exit_code {
        2 => DesktopAppleSpeechErrorCode::UnsupportedLocale,
        3 => DesktopAppleSpeechErrorCode::Unavailable,
        _ => DesktopAppleSpeechErrorCode::Failed,
    }
}

/// Converts 16 kHz mono `f32le` PCM bytes into 16-bit `RIFF`/`WAVE` for `AVAudioFile` (`AppleSpeech.ts:30-65`).
pub fn encode_apple_speech_wav(pcm_f32le: &[u8]) -> Result<Vec<u8>, DesktopAppleSpeechErrorCode> {
    if pcm_f32le.is_empty()
        || pcm_f32le.len() > MAX_APPLE_SPEECH_PCM_BYTES
        || !pcm_f32le.len().is_multiple_of(4)
    {
        return Err(DesktopAppleSpeechErrorCode::Failed);
    }
    let (quads, _) = pcm_f32le.as_chunks::<4>();
    let mut samples = Vec::with_capacity(quads.len());
    for quad in quads {
        let raw = f32::from_le_bytes(*quad);
        samples.push(if raw.is_finite() {
            raw.clamp(-1.0_f32, 1.0_f32)
        } else {
            0.0_f32
        });
    }
    encode_wav_pcm16le(&samples, APPLE_SPEECH_SAMPLE_RATE_HZ, 1)
        .map_err(|_| DesktopAppleSpeechErrorCode::Failed)
}
