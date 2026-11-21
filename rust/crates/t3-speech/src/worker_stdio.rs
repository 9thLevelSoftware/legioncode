//! Synchronous bounded `BufRead` / `Write` `NDJSON` execution loop for `t3 __speech-worker`.

use std::io::{BufRead, Write};

use crate::asr::{AsrEngine, AsrSessionState, AsrTranscriberSession};
use crate::types::TranscriptionOptions;
use crate::worker_framing::{
    BoundedLine, emit_asr_error, emit_event, emit_ready, read_bounded_line,
};
pub use crate::worker_framing::{WorkerExitReason, WorkerIoError};
use crate::worker_protocol::{
    DEFAULT_MAX_WORKER_LINE_BYTES, WorkerAudioPayload, WorkerCommand, WorkerErrorCode, WorkerEvent,
    decode_worker_command,
};

/// Runs the synchronous `t3 __speech-worker` `NDJSON` loop over generic `BufRead` / `Write` streams.
pub fn run_worker_stdio<R: BufRead, W: Write, E: AsrEngine>(
    reader: R,
    writer: W,
    session: &mut AsrTranscriberSession<E>,
) -> Result<WorkerExitReason, WorkerIoError> {
    run_worker_stdio_with_max_line(reader, writer, session, DEFAULT_MAX_WORKER_LINE_BYTES)
}

/// Runs [`run_worker_stdio`] with an explicit per-line byte ceiling for bounding memory.
pub fn run_worker_stdio_with_max_line<R: BufRead, W: Write, E: AsrEngine>(
    mut reader: R,
    mut writer: W,
    session: &mut AsrTranscriberSession<E>,
    max_line_bytes: usize,
) -> Result<WorkerExitReason, WorkerIoError> {
    let mut default_options = TranscriptionOptions::default();

    loop {
        match read_bounded_line(&mut reader, max_line_bytes)? {
            BoundedLine::Eof => {
                let _ = session.reset();
                writer.flush()?;
                return Ok(WorkerExitReason::Eof);
            }
            BoundedLine::TooLarge { observed_bytes } => {
                emit_event(
                    &mut writer,
                    &WorkerEvent::Error {
                        code: WorkerErrorCode::FrameTooLarge,
                        message: format!(
                            "Worker command line of {observed_bytes} bytes exceeds {max_line_bytes} byte limit."
                        ),
                        generation_id: session.vad_generation_id(),
                        reset_performed: false,
                    },
                    max_line_bytes,
                )?;
            }
            BoundedLine::Line(raw_bytes) => {
                let Ok(text) = std::str::from_utf8(&raw_bytes) else {
                    emit_event(
                        &mut writer,
                        &WorkerEvent::Error {
                            code: WorkerErrorCode::InvalidCommand,
                            message: "Worker command line is not valid UTF-8.".to_owned(),
                            generation_id: session.vad_generation_id(),
                            reset_performed: false,
                        },
                        max_line_bytes,
                    )?;
                    continue;
                };
                if text.trim().is_empty() {
                    continue;
                }
                let command = match decode_worker_command(text) {
                    Ok(cmd) => cmd,
                    Err(err) => {
                        emit_event(
                            &mut writer,
                            &WorkerEvent::Error {
                                code: WorkerErrorCode::InvalidCommand,
                                message: err.to_string(),
                                generation_id: session.vad_generation_id(),
                                reset_performed: false,
                            },
                            max_line_bytes,
                        )?;
                        continue;
                    }
                };
                if handle_command(
                    command,
                    &mut default_options,
                    session,
                    &mut writer,
                    max_line_bytes,
                )? {
                    writer.flush()?;
                    return Ok(WorkerExitReason::Shutdown);
                }
            }
        }
    }
}

fn handle_command<W: Write, E: AsrEngine>(
    command: WorkerCommand,
    default_options: &mut TranscriptionOptions,
    session: &mut AsrTranscriberSession<E>,
    writer: &mut W,
    max_line_bytes: usize,
) -> Result<bool, WorkerIoError> {
    match command {
        WorkerCommand::Config {
            options,
            streaming,
            model_path: _,
            vad_path: _,
        } => {
            *default_options = options;
            if let Err(err) = session.reset() {
                emit_asr_error(writer, session, &err, max_line_bytes)?;
                return Ok(false);
            }
            if streaming && let Err(err) = session.begin_stream(default_options.clone()) {
                emit_asr_error(writer, session, &err, max_line_bytes)?;
                return Ok(false);
            }
            emit_ready(writer, session, max_line_bytes)?;
        }
        WorkerCommand::AudioChunk { audio, final_chunk } => {
            handle_audio_chunk(
                &audio,
                final_chunk,
                default_options,
                session,
                writer,
                max_line_bytes,
            )?;
        }
        WorkerCommand::Transcribe { audio, options } => {
            let opts = options.as_ref().unwrap_or(default_options);
            handle_transcribe(&audio, opts, session, writer, max_line_bytes)?;
        }
        WorkerCommand::Reset => {
            if let Err(err) = session.reset() {
                emit_asr_error(writer, session, &err, max_line_bytes)?;
                return Ok(false);
            }
            emit_ready(writer, session, max_line_bytes)?;
        }
        WorkerCommand::Shutdown => {
            let _ = session.reset();
            emit_event(
                writer,
                &WorkerEvent::Stopped {
                    reason: "shutdown".to_owned(),
                    generation_id: session.vad_generation_id(),
                },
                max_line_bytes,
            )?;
            return Ok(true);
        }
    }
    Ok(false)
}

fn handle_audio_chunk<W: Write, E: AsrEngine>(
    audio: &WorkerAudioPayload,
    final_chunk: bool,
    default_options: &TranscriptionOptions,
    session: &mut AsrTranscriberSession<E>,
    writer: &mut W,
    max_line_bytes: usize,
) -> Result<(), WorkerIoError> {
    let samples = match audio.decode_to_f32() {
        Ok(samples) => samples,
        Err(err) => {
            let _ = session.reset();
            emit_event(
                writer,
                &WorkerEvent::Error {
                    code: WorkerErrorCode::InvalidAudio,
                    message: err.to_string(),
                    generation_id: session.vad_generation_id(),
                    reset_performed: true,
                },
                max_line_bytes,
            )?;
            return Ok(());
        }
    };

    if session.state() == AsrSessionState::Idle
        && let Err(err) = session.begin_stream(default_options.clone())
    {
        emit_asr_error(writer, session, &err, max_line_bytes)?;
        return Ok(());
    }

    match session.feed_stream_f32(&samples) {
        Ok(feed) => {
            for decision in feed.vad_decisions {
                emit_event(
                    writer,
                    &WorkerEvent::Vad {
                        generation_id: feed.generation_id,
                        decision,
                    },
                    max_line_bytes,
                )?;
            }
            if let Some(preview) = feed.preview {
                emit_event(
                    writer,
                    &WorkerEvent::Transcript {
                        generation_id: feed.generation_id,
                        text: preview.full_text(),
                        committed: preview.committed,
                        tentative: preview.tentative,
                        is_final: false,
                    },
                    max_line_bytes,
                )?;
            }
        }
        Err(err) => {
            emit_asr_error(writer, session, &err, max_line_bytes)?;
            return Ok(());
        }
    }

    if final_chunk {
        match session.finish_stream() {
            Ok(finished) => {
                for decision in finished.vad_decisions {
                    emit_event(
                        writer,
                        &WorkerEvent::Vad {
                            generation_id: finished.generation_id,
                            decision,
                        },
                        max_line_bytes,
                    )?;
                }
                emit_event(
                    writer,
                    &WorkerEvent::Transcript {
                        generation_id: finished.generation_id,
                        text: finished.text.clone(),
                        committed: finished.text,
                        tentative: String::new(),
                        is_final: true,
                    },
                    max_line_bytes,
                )?;
            }
            Err(err) => {
                emit_asr_error(writer, session, &err, max_line_bytes)?;
            }
        }
    }
    Ok(())
}

fn handle_transcribe<W: Write, E: AsrEngine>(
    audio: &WorkerAudioPayload,
    options: &TranscriptionOptions,
    session: &mut AsrTranscriberSession<E>,
    writer: &mut W,
    max_line_bytes: usize,
) -> Result<(), WorkerIoError> {
    let samples = match audio.decode_to_f32() {
        Ok(samples) => samples,
        Err(err) => {
            let _ = session.reset();
            emit_event(
                writer,
                &WorkerEvent::Error {
                    code: WorkerErrorCode::InvalidAudio,
                    message: err.to_string(),
                    generation_id: session.vad_generation_id(),
                    reset_performed: true,
                },
                max_line_bytes,
            )?;
            return Ok(());
        }
    };

    match session.transcribe_f32(&samples, options) {
        Ok(outcome) => {
            for decision in outcome.vad_decisions {
                emit_event(
                    writer,
                    &WorkerEvent::Vad {
                        generation_id: outcome.generation_id,
                        decision,
                    },
                    max_line_bytes,
                )?;
            }
            emit_event(
                writer,
                &WorkerEvent::Transcript {
                    generation_id: outcome.generation_id,
                    text: outcome.text.clone(),
                    committed: outcome.text,
                    tentative: String::new(),
                    is_final: true,
                },
                max_line_bytes,
            )?;
        }
        Err(err) => {
            emit_asr_error(writer, session, &err, max_line_bytes)?;
        }
    }
    Ok(())
}
