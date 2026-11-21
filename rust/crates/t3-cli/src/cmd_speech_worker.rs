//! Synchronous `t3 __speech-worker` NDJSON stdio worker loop and native discovery commands backed
//! by `t3_speech` (`AcousticAsrEngine` + `t3_speech::native_discovery`).

use std::env;
use std::path::Path;

use t3_speech::acoustic_model::{AcousticAsrEngine, AcousticModelWeights};
use t3_speech::asr::AsrTranscriberSession;
use t3_speech::native_discovery::{discover_speech_devices, inspect_model_file};
use t3_speech::vad::VadConfig;
use t3_speech::worker_protocol::run_worker_stdio;

use crate::cli_global::fail_cli_error;
use crate::fallback::{delegate_to_node, unit_forced_original, write_stdout_line};

/// Unit identifier for `RUST_ANYTHING_UNIT_SPEECH_WORKER` and fallback logs.
pub const UNIT_SPEECH_WORKER: &str = "SPEECH_WORKER";

/// Builds the active [`AcousticAsrEngine`] for `__speech-worker` and `/ws/voice`.
pub(crate) fn build_acoustic_engine() -> AcousticAsrEngine {
    if let Ok(path) = env::var("T3CODE_SPEECH_MODEL_PATH") {
        let trimmed = path.trim();
        if !trimmed.is_empty()
            && let Ok(weights) = AcousticModelWeights::load_from_file(Path::new(trimmed))
            && let Ok(engine) = AcousticAsrEngine::new(weights)
        {
            return engine;
        }
    }
    AcousticAsrEngine::with_default_english()
}

/// Runs `t3 __speech-worker`, handling `--stdio`, `discover`, and `model <path>` natively.
pub fn run(args: &[String]) -> i32 {
    if unit_forced_original(UNIT_SPEECH_WORKER) {
        return delegate_to_node(UNIT_SPEECH_WORKER, args);
    }

    let rest = args.get(1..).unwrap_or(&[]);
    match rest {
        [] => run_stdio(),
        [single] if single == "--stdio" => run_stdio(),
        [sub, ..] if sub == "discover" => run_discover(&rest[1..]),
        [sub, ..] if sub == "model" => run_model(&rest[1..]),
        [other, ..] => fail_cli_error(&format!(
            "Unknown __speech-worker mode \"{other}\". Expected `--stdio`, `discover`, or `model <path>`."
        )),
    }
}

fn run_stdio() -> i32 {
    let mut session =
        match AsrTranscriberSession::new(build_acoustic_engine(), VadConfig::default()) {
            Ok(session) => session,
            Err(err) => {
                eprintln!("Failed to initialize speech worker VAD session: {err}");
                return 1;
            }
        };

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    match run_worker_stdio(stdin.lock(), stdout.lock(), &mut session) {
        Ok(_) => 0,
        Err(err) => {
            eprintln!("Speech worker stdio loop failed: {err}");
            1
        }
    }
}

fn run_discover(extra: &[String]) -> i32 {
    if extra.len() > 1 {
        return fail_cli_error(&format!(
            "Unexpected argument `{}` for `__speech-worker discover`.",
            extra[1]
        ));
    }
    let devices = discover_speech_devices();
    match serde_json::to_string_pretty(&devices) {
        Ok(json) => write_stdout_line(&json, "speech device discovery"),
        Err(err) => {
            eprintln!("Failed to serialize speech device discovery output: {err}");
            1
        }
    }
}

fn run_model(tail: &[String]) -> i32 {
    let Some(path) = tail.first().map(String::as_str) else {
        return fail_cli_error("Missing required argument: path");
    };
    if tail.len() > 2 {
        return fail_cli_error(&format!(
            "Unexpected argument `{}` for `__speech-worker model`.",
            tail[2]
        ));
    }
    match inspect_model_file(Path::new(path)) {
        Ok(info) => match serde_json::to_string_pretty(&info) {
            Ok(json) => write_stdout_line(&json, "speech model inspection"),
            Err(err) => {
                eprintln!("Failed to serialize speech model inspection: {err}");
                1
            }
        },
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}
