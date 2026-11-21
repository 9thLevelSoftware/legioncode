//! Bounded `NDJSON` line reading, exit/error types, and event emitters for `t3 __speech-worker`.

use std::io::{self, BufRead, Write};

use thiserror::Error;

use crate::asr::{AsrEngine, AsrError, AsrTranscriberSession};
use crate::types::SPEECH_SAMPLE_RATE_HZ;
use crate::worker_protocol::{WorkerErrorCode, WorkerEvent, encode_ndjson_line};

/// Reason [`crate::worker_protocol::run_worker_stdio`] exited cleanly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerExitReason {
    /// Client issued an explicit [`crate::worker_protocol::WorkerCommand::Shutdown`].
    Shutdown,
    /// Input stream reached `EOF`.
    Eof,
}

/// I/O or protocol-write failure raised by [`crate::worker_protocol::run_worker_stdio`].
#[derive(Debug, Error)]
pub enum WorkerIoError {
    /// Underlying reader or writer returned an I/O error.
    #[error("worker stdio I/O error: {0}")]
    Io(#[from] io::Error),
    /// Outbound [`WorkerEvent`] serialization exceeded limits.
    #[error("worker event serialization failed: {0}")]
    Encode(#[from] crate::worker_protocol::WorkerProtocolError),
}

pub(crate) enum BoundedLine {
    Eof,
    Line(Vec<u8>),
    TooLarge { observed_bytes: usize },
}

pub(crate) fn emit_ready<W: Write, E: AsrEngine>(
    writer: &mut W,
    session: &AsrTranscriberSession<E>,
    max_line_bytes: usize,
) -> Result<(), WorkerIoError> {
    emit_event(
        writer,
        &WorkerEvent::Ready {
            generation_id: session.vad_generation_id(),
            sample_rate_hz: SPEECH_SAMPLE_RATE_HZ,
            frame_samples: session.vad().accumulator().frame_samples(),
        },
        max_line_bytes,
    )
}

pub(crate) fn emit_asr_error<W: Write, E: AsrEngine>(
    writer: &mut W,
    session: &AsrTranscriberSession<E>,
    err: &AsrError,
    max_line_bytes: usize,
) -> Result<(), WorkerIoError> {
    let reset_performed = !matches!(err, AsrError::InvalidState { .. });
    let code = match err {
        AsrError::Vad(_) => WorkerErrorCode::VadFailure,
        AsrError::InvalidState { .. } => WorkerErrorCode::InvalidState,
        AsrError::Engine { .. } => WorkerErrorCode::AsrFailure,
    };
    emit_event(
        writer,
        &WorkerEvent::Error {
            code,
            message: err.to_string(),
            generation_id: session.vad_generation_id(),
            reset_performed,
        },
        max_line_bytes,
    )
}

pub(crate) fn emit_event<W: Write>(
    writer: &mut W,
    event: &WorkerEvent,
    max_line_bytes: usize,
) -> Result<(), WorkerIoError> {
    let line = encode_ndjson_line(event, max_line_bytes)?;
    writer.write_all(line.as_bytes())?;
    writer.flush()?;
    Ok(())
}

pub(crate) fn read_bounded_line<R: BufRead>(
    reader: &mut R,
    max_bytes: usize,
) -> Result<BoundedLine, io::Error> {
    let mut buf = Vec::new();
    let mut total_seen = 0usize;
    let mut oversized = false;

    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            if total_seen == 0 {
                return Ok(BoundedLine::Eof);
            }
            if oversized {
                return Ok(BoundedLine::TooLarge {
                    observed_bytes: total_seen,
                });
            }
            if buf.ends_with(b"\r") {
                buf.pop();
            }
            return Ok(BoundedLine::Line(buf));
        }

        if let Some(rel_nl) = available.iter().position(|&b| b == b'\n') {
            let segment = &available[..rel_nl];
            total_seen = total_seen.saturating_add(segment.len());
            if oversized || total_seen > max_bytes {
                reader.consume(rel_nl + 1);
                return Ok(BoundedLine::TooLarge {
                    observed_bytes: total_seen,
                });
            }
            buf.extend_from_slice(segment);
            reader.consume(rel_nl + 1);
            if buf.ends_with(b"\r") {
                buf.pop();
            }
            return Ok(BoundedLine::Line(buf));
        }

        total_seen = total_seen.saturating_add(available.len());
        if total_seen > max_bytes {
            oversized = true;
            buf.clear();
        } else if !oversized {
            buf.extend_from_slice(available);
        }
        let consumed = available.len();
        reader.consume(consumed);
    }
}
