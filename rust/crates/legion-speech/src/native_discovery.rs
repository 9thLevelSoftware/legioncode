//! Safe Rust backend discovery and model inspection for `legioncode __speech-worker discover|model`.

use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::acoustic_weights::{AcousticModelError, AcousticModelWeights, LCASR_BINARY_MAGIC};

const GGUF_MAGIC: &[u8; 4] = b"GGUF";

/// Compute backend driver family reported by [`discover_speech_devices`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeechBackendKind {
    Cpu,
    Metal,
    DirectMl,
    Vulkan,
    Cuda,
}

/// Hardware device category for a discovered speech backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeechDeviceType {
    Cpu,
    Gpu,
    Igpu,
}

/// Device descriptor emitted by `legioncode __speech-worker discover`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeechDeviceInfo {
    /// Deterministic backend ID matching `[kind, deviceId]` JSON formatting.
    pub id: String,
    /// Human-readable device label.
    pub name: String,
    /// Backend driver family.
    pub kind: SpeechBackendKind,
    /// Hardware device class (`cpu`, `gpu`, or `igpu`).
    pub device_type: SpeechDeviceType,
    /// Whether this backend is ready for in-process Safe Rust inference.
    pub available: bool,
}

/// Recognized local speech model container format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeechModelFormat {
    LcAsrJson,
    LcAsrBinary,
    Gguf,
}

/// Core feature capability flags reported for an inspected speech model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeechModelCapabilities {
    /// Supports incremental live streaming previews.
    pub streaming: bool,
    /// Supports long-form / midpoint-split inference.
    pub long_form: bool,
    /// Supports custom vocabulary alias / hint matching.
    pub vocabulary: bool,
}

/// Metadata and capability summary emitted by `legioncode __speech-worker model [path]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeechModelInspection {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub format: SpeechModelFormat,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    pub prototype_count: usize,
    pub capabilities: SpeechModelCapabilities,
    pub supports_translation: bool,
}

/// Errors returned by [`inspect_model_file`].
#[derive(Debug, Clone, PartialEq, Error)]
pub enum ModelInspectionError {
    #[error("cannot inspect model `{path}`: {message}")]
    Io { path: String, message: String },
    #[error("model path `{path}` is not a regular file")]
    NotAFile { path: String },
    #[error("unsupported or corrupt speech model `{path}`: {reason}")]
    UnsupportedFormat { path: String, reason: String },
}

/// Discovers native speech execution backends available on the current host.
pub fn discover_speech_devices() -> Vec<SpeechDeviceInfo> {
    let threads = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    let mut devices = vec![SpeechDeviceInfo {
        id: r#"["cpu","cpu0"]"#.to_owned(),
        name: format!(
            "Safe Rust DSP CPU ({}-{}, {} threads)",
            std::env::consts::OS,
            std::env::consts::ARCH,
            threads
        ),
        kind: SpeechBackendKind::Cpu,
        device_type: SpeechDeviceType::Cpu,
        available: true,
    }];
    if cfg!(target_os = "macos") && cfg!(target_arch = "aarch64") {
        devices.push(SpeechDeviceInfo {
            id: r#"["metal","apple-silero-dsp"]"#.to_owned(),
            name: "Apple Silicon Unified Acceleration (Safe Rust Fallback)".to_owned(),
            kind: SpeechBackendKind::Metal,
            device_type: SpeechDeviceType::Igpu,
            available: false,
        });
    } else if cfg!(target_os = "linux") && Path::new("/dev/dri/renderD128").exists() {
        devices.push(SpeechDeviceInfo {
            id: r#"["vulkan","renderD128"]"#.to_owned(),
            name: "Linux DRM Render Node (CPU Fallback Active)".to_owned(),
            kind: SpeechBackendKind::Vulkan,
            device_type: SpeechDeviceType::Gpu,
            available: false,
        });
    }
    devices
}

/// Inspects a local `.lcasr` (JSON or `LCASR1` binary) or `.gguf` speech model file.
pub fn inspect_model_file(path: &Path) -> Result<SpeechModelInspection, ModelInspectionError> {
    let path_str = path.display().to_string();
    let io_err = |err: std::io::Error| ModelInspectionError::Io {
        path: path_str.clone(),
        message: err.to_string(),
    };
    let meta = fs::metadata(path).map_err(io_err)?;
    if !meta.is_file() {
        return Err(ModelInspectionError::NotAFile { path: path_str });
    }
    let mut header = [0_u8; 8];
    let n = File::open(path)
        .and_then(|mut f| f.read(&mut header))
        .map_err(io_err)?;
    let capabilities = SpeechModelCapabilities {
        streaming: true,
        long_form: true,
        vocabulary: true,
    };
    if n >= 8 && &header[0..4] == GGUF_MAGIC {
        let ver = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
        if !(1..=3).contains(&ver) {
            return Err(ModelInspectionError::UnsupportedFormat {
                path: path_str,
                reason: format!("unsupported GGUF version {ver}"),
            });
        }
        return Ok(SpeechModelInspection {
            path: path.to_path_buf(),
            size_bytes: meta.len(),
            format: SpeechModelFormat::Gguf,
            model_id: path.file_stem().and_then(|s| s.to_str()).map(str::to_owned),
            language: None,
            prototype_count: 0,
            capabilities,
            supports_translation: false,
        });
    }
    let weights = AcousticModelWeights::load_from_file(path).map_err(|err| match err {
        AcousticModelError::Io { message } => ModelInspectionError::Io {
            path: path_str,
            message,
        },
        other => ModelInspectionError::UnsupportedFormat {
            path: path_str,
            reason: other.to_string(),
        },
    })?;
    let format = if n >= 8 && &header == LCASR_BINARY_MAGIC {
        SpeechModelFormat::LcAsrBinary
    } else {
        SpeechModelFormat::LcAsrJson
    };
    Ok(SpeechModelInspection {
        path: path.to_path_buf(),
        size_bytes: meta.len(),
        format,
        model_id: Some(weights.model_id),
        language: Some(weights.language),
        prototype_count: weights.prototypes.len(),
        capabilities,
        supports_translation: false,
    })
}
