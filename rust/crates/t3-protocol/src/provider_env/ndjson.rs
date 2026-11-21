//! LF-only 128 MiB streaming NDJSON line framing codec.
//!
//! Mirrors `packages/effect-codex-app-server/src/protocol.ts` and
//! `packages/provider-pi/src/server/rpc.ts`: splits strictly on `\n` (never bare
//! `\r`, `U+2028`, or `U+2029`), strips an optional single `\r` immediately before
//! `\n`, skips blank separator lines, and bounds memory at [`MAX_NDJSON_LINE_BYTES`].

use serde::Serialize;
use serde::de::DeserializeOwned;
use thiserror::Error;

/// Default maximum permitted NDJSON line length (`128 MiB`).
pub const MAX_NDJSON_LINE_BYTES: usize = 128 * 1024 * 1024;

/// Errors raised by [`NdjsonLineCodec`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum NdjsonCodecError {
    /// Buffered or completed line exceeded the configured byte ceiling.
    #[error("NDJSON line exceeded maximum size of {max_bytes} bytes (saw {observed_bytes} bytes)")]
    LineTooLarge {
        /// Configured byte ceiling.
        max_bytes: usize,
        /// Observed size when the cap was breached.
        observed_bytes: usize,
    },
    /// Completed line did not contain valid UTF-8.
    #[error("NDJSON line is not valid UTF-8: {0}")]
    InvalidUtf8(#[from] std::str::Utf8Error),
    /// JSON serialization or deserialization failed.
    #[error("NDJSON JSON codec error: {message}")]
    Json {
        /// Parser or serializer message.
        message: String,
    },
}

/// Incremental LF-only NDJSON line decoder/encoder with strict byte bounds.
#[derive(Debug, Clone)]
pub struct NdjsonLineCodec {
    max_line_bytes: usize,
    buffer: Vec<u8>,
}

impl Default for NdjsonLineCodec {
    fn default() -> Self {
        Self::new(MAX_NDJSON_LINE_BYTES)
    }
}

impl NdjsonLineCodec {
    /// Creates an [`NdjsonLineCodec`] bounded to `max_line_bytes` per line.
    pub fn new(max_line_bytes: usize) -> Self {
        Self {
            max_line_bytes,
            buffer: Vec::new(),
        }
    }

    /// Returns the configured maximum bytes per line.
    pub fn max_line_bytes(&self) -> usize {
        self.max_line_bytes
    }

    /// Returns the number of buffered partial-line bytes currently pending `\n`.
    pub fn pending_bytes(&self) -> usize {
        self.buffer.len()
    }

    /// Feeds raw stdout bytes and emits every non-empty completed LF-terminated line.
    ///
    /// Fails immediately with [`NdjsonCodecError::LineTooLarge`] if any line (completed
    /// or unterminated) exceeds `max_line_bytes`, clearing the partial buffer.
    pub fn push_bytes(&mut self, chunk: &[u8]) -> Result<Vec<String>, NdjsonCodecError> {
        let mut lines = Vec::new();
        let mut start = 0usize;

        while let Some(rel_nl) = chunk[start..].iter().position(|&b| b == b'\n') {
            let end = start + rel_nl;
            let segment = &chunk[start..end];
            let candidate_len = self.buffer.len().saturating_add(segment.len());
            if candidate_len > self.max_line_bytes {
                self.buffer.clear();
                return Err(NdjsonCodecError::LineTooLarge {
                    max_bytes: self.max_line_bytes,
                    observed_bytes: candidate_len,
                });
            }

            let raw_line = if self.buffer.is_empty() {
                strip_trailing_cr(segment)
            } else {
                self.buffer.extend_from_slice(segment);
                let stripped = strip_trailing_cr(&self.buffer);
                let text = std::str::from_utf8(stripped).map(str::to_owned);
                self.buffer.clear();
                let owned = text?;
                if !owned.trim().is_empty() {
                    lines.push(owned);
                }
                start = end + 1;
                continue;
            };

            if !raw_line.is_empty() {
                let text = std::str::from_utf8(raw_line)?;
                if !text.trim().is_empty() {
                    lines.push(text.to_owned());
                }
            }
            start = end + 1;
        }

        let remainder = &chunk[start..];
        let new_pending = self.buffer.len().saturating_add(remainder.len());
        if new_pending > self.max_line_bytes {
            self.buffer.clear();
            return Err(NdjsonCodecError::LineTooLarge {
                max_bytes: self.max_line_bytes,
                observed_bytes: new_pending,
            });
        }
        self.buffer.extend_from_slice(remainder);
        Ok(lines)
    }

    /// Flushes any trailing unterminated non-blank line buffered at EOF.
    pub fn finish(&mut self) -> Result<Option<String>, NdjsonCodecError> {
        if self.buffer.is_empty() {
            return Ok(None);
        }
        let stripped = strip_trailing_cr(&self.buffer);
        let text = std::str::from_utf8(stripped)?.to_owned();
        self.buffer.clear();
        if text.trim().is_empty() {
            Ok(None)
        } else {
            Ok(Some(text))
        }
    }

    /// Feeds raw stdout bytes and deserializes every completed line into `T`.
    pub fn push_json_lines<T: DeserializeOwned>(
        &mut self,
        chunk: &[u8],
    ) -> Result<Vec<T>, NdjsonCodecError> {
        let raw_lines = self.push_bytes(chunk)?;
        raw_lines
            .into_iter()
            .map(|line| {
                serde_json::from_str::<T>(&line).map_err(|err| NdjsonCodecError::Json {
                    message: err.to_string(),
                })
            })
            .collect()
    }

    /// Serializes `value` as a compact single-line NDJSON frame ending in `\n`.
    pub fn encode_line<T: Serialize>(&self, value: &T) -> Result<String, NdjsonCodecError> {
        let mut encoded = serde_json::to_string(value).map_err(|err| NdjsonCodecError::Json {
            message: err.to_string(),
        })?;
        if encoded.len() > self.max_line_bytes {
            return Err(NdjsonCodecError::LineTooLarge {
                max_bytes: self.max_line_bytes,
                observed_bytes: encoded.len(),
            });
        }
        encoded.push('\n');
        Ok(encoded)
    }
}

fn strip_trailing_cr(line: &[u8]) -> &[u8] {
    line.strip_suffix(b"\r").unwrap_or(line)
}
