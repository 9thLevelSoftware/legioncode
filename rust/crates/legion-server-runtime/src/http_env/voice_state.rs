//! Voice runtime state, model catalog, device discovery status, and shared error helpers.

use std::collections::HashSet;
use std::path::PathBuf;

use axum::http::StatusCode;
use axum::response::Response;
use legion_speech::discover_speech_devices;
use serde::Serialize;
use serde_json::{Value, json};

use crate::http_env::{EnvErr, EnvResult, env_bad_request, env_error_json};

const DEFAULT_MODEL_ID: &str = "parakeet-tdt-0.6b-v3";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[expect(
    clippy::struct_excessive_bools,
    reason = "mirrors EnvironmentSpeechModel wire fields in packages/contracts/src/speech.ts"
)]
pub(crate) struct VoiceModelEntry {
    pub id: String,
    pub name: String,
    pub description: String,
    pub size: u64,
    pub languages: Vec<String>,
    pub accuracy: f64,
    pub speed: f64,
    pub recommended: bool,
    pub supports_streaming: bool,
    pub supports_language_detection: bool,
    pub supports_translation: bool,
    pub active: bool,
    pub state: String,
}

/// Thread-safe voice status, model catalog, and cloud key state stored in `RuntimeHttpState`.
#[derive(Debug, Clone)]
pub(crate) struct VoiceRuntimeState {
    pub active_model_id: String,
    pub acceleration: String,
    pub model_unload_timeout: String,
    pub installed_models: HashSet<String>,
    pub openai_key: Option<String>,
    pub xai_key: Option<String>,
    pub openrouter_key: Option<String>,
    pub linked_paths: Vec<PathBuf>,
}

impl Default for VoiceRuntimeState {
    fn default() -> Self {
        Self {
            active_model_id: DEFAULT_MODEL_ID.to_owned(),
            acceleration: "auto".to_owned(),
            model_unload_timeout: "min_5".to_owned(),
            installed_models: HashSet::new(),
            openai_key: None,
            xai_key: None,
            openrouter_key: None,
            linked_paths: Vec::new(),
        }
    }
}

impl VoiceRuntimeState {
    pub(crate) fn mark_default_linked(&mut self, path: PathBuf) {
        self.linked_paths.push(path);
        self.installed_models.insert(DEFAULT_MODEL_ID.to_owned());
    }

    pub(crate) fn catalog(&self) -> Vec<VoiceModelEntry> {
        let specs = [
            (
                "parakeet-tdt-0.6b-v3",
                "Parakeet TDT 0.6B v3",
                "Fast high-accuracy English & European speech model",
                670_000_000_u64,
                vec![
                    "en".to_owned(),
                    "fr".to_owned(),
                    "de".to_owned(),
                    "es".to_owned(),
                ],
                0.95,
                0.96,
                true,
                true,
                false,
                false,
            ),
            (
                "whisper-large-v3-turbo",
                "Whisper Large v3 Turbo",
                "Multilingual transcription & translation model",
                809_000_000_u64,
                vec!["en".to_owned(), "multilingual".to_owned()],
                0.96,
                0.82,
                false,
                false,
                true,
                true,
            ),
        ];
        specs
            .into_iter()
            .map(
                |(id, name, desc, size, langs, acc, spd, rec, stream, lang_det, trans)| {
                    let installed = self.installed_models.contains(id);
                    VoiceModelEntry {
                        id: id.to_owned(),
                        name: name.to_owned(),
                        description: desc.to_owned(),
                        size,
                        languages: langs,
                        accuracy: acc,
                        speed: spd,
                        recommended: rec,
                        supports_streaming: stream,
                        supports_language_detection: lang_det,
                        supports_translation: trans,
                        active: self.active_model_id == id,
                        state: if installed {
                            "installed".to_owned()
                        } else {
                            "downloadable".to_owned()
                        },
                    }
                },
            )
            .collect()
    }

    pub(crate) fn status_json(&self) -> Value {
        let devices = discover_speech_devices();
        let active_entry = self
            .catalog()
            .into_iter()
            .find(|m| m.id == self.active_model_id)
            .unwrap_or_else(|| self.catalog()[0].clone());
        let state_str = if self.installed_models.contains(&self.active_model_id) {
            "ready"
        } else {
            "missing-model"
        };
        let gpu_devices: Vec<Value> = devices
            .iter()
            .filter(|d| !d.name.eq_ignore_ascii_case("cpu"))
            .enumerate()
            .map(|(idx, d)| json!({ "id": format!("gpu:{idx}"), "name": d.name }))
            .collect();
        let tier = if self.acceleration == "cpu"
            || (self.acceleration == "auto" && gpu_devices.is_empty())
        {
            "cpu"
        } else {
            "accelerated"
        };

        json!({
            "supported": true,
            "state": state_str,
            "modelId": active_entry.id,
            "model": active_entry.name,
            "size": active_entry.size,
            "supportsStreaming": active_entry.supports_streaming,
            "supportsBufferedStreaming": true,
            "supportsModelLinking": true,
            "supportsTranslation": active_entry.supports_translation,
            "languages": active_entry.languages,
            "supportsLanguageDetection": active_entry.supports_language_detection,
            "acceleration": self.acceleration,
            "benchmarkTier": tier,
            "modelUnloadTimeout": self.model_unload_timeout,
            "gpuDevices": gpu_devices,
        })
    }
}

pub(crate) fn cloud_speech_error(
    provider: &str,
    reason: &str,
    message: impl Into<String>,
) -> Response {
    env_error_json(
        StatusCode::UNPROCESSABLE_ENTITY,
        json!({
            "_tag": "CloudSpeechError",
            "provider": provider,
            "reason": reason,
            "message": message.into(),
        }),
    )
}

pub(crate) fn valid_cloud_provider(provider: &str) -> EnvResult<&str> {
    match provider {
        "openai" | "xai" | "openrouter" => Ok(provider),
        other => Err(EnvErr(Box::new(env_bad_request(format!(
            "Unsupported cloud speech provider `{other}`"
        ))))),
    }
}
