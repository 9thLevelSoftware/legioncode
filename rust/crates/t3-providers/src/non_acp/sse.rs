//! Byte-chunked RFC/WHATWG Server-Sent Events (SSE) frame parser.

use serde::{Deserialize, Serialize};

use super::NonAcpParseError;

/// Single dispatched SSE frame emitted after a blank line separator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SseEventFrame {
    /// Optional `event:` field value.
    pub event: Option<String>,
    /// Optional `id:` field value.
    pub id: Option<String>,
    /// Multi-line `data:` lines joined by `\n`.
    pub data: String,
    /// `true` when `data.trim() == "[DONE]"` (standard streaming terminator).
    pub is_done: bool,
}

/// Byte-chunked RFC/WHATWG Server-Sent Events frame decoder (`[DONE]` aware).
///
/// Buffers incomplete bytes across arbitrary transport reads so multi-byte UTF-8
/// characters inside JSON payloads are never split mid-sequence.
///
/// # Examples
///
/// ```
/// use t3_providers::non_acp::SseStreamParser;
///
/// let mut sse = SseStreamParser::new();
/// assert!(sse.push_bytes(b"data: {\"hello\":")?.is_empty());
/// let frames = sse.push_bytes(b"\"world\"}\n\ndata: [DONE]\n\n")?;
/// assert_eq!(frames.len(), 2);
/// assert!(!frames[0].is_done);
/// assert!(frames[1].is_done);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, Default)]
pub struct SseStreamParser {
    byte_buf: Vec<u8>,
    event: Option<String>,
    id: Option<String>,
    data_lines: Vec<String>,
}

impl SseStreamParser {
    /// Creates an empty incremental SSE parser.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds raw HTTP response body bytes and emits all completed [`SseEventFrame`] items.
    pub fn push_bytes(&mut self, bytes: &[u8]) -> Result<Vec<SseEventFrame>, NonAcpParseError> {
        self.byte_buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        while let Some(pos) = self.byte_buf.iter().position(|&b| b == b'\n') {
            let raw_line = self.byte_buf.drain(..=pos).collect::<Vec<u8>>();
            let line = decode_sse_line(&raw_line)?;
            if let Some(frame) = self.process_line(line) {
                out.push(frame);
            }
        }
        Ok(out)
    }

    /// Flushes any pending trailing line/event when the HTTP body reaches EOF.
    pub fn finish(&mut self) -> Result<Vec<SseEventFrame>, NonAcpParseError> {
        let mut out = Vec::new();
        if !self.byte_buf.is_empty() {
            let raw = std::mem::take(&mut self.byte_buf);
            let line = decode_sse_line(&raw)?;
            if let Some(frame) = self.process_line(line) {
                out.push(frame);
            }
        }
        if let Some(frame) = self.flush_event() {
            out.push(frame);
        }
        Ok(out)
    }

    fn process_line(&mut self, line: &str) -> Option<SseEventFrame> {
        if line.is_empty() {
            return self.flush_event();
        }
        if line.starts_with(':') {
            return None;
        }
        let (field, raw_val) = match line.split_once(':') {
            Some((k, v)) => (k, v.strip_prefix(' ').unwrap_or(v)),
            None => (line, ""),
        };
        match field {
            "data" => self.data_lines.push(raw_val.to_owned()),
            "event" => self.event = Some(raw_val.to_owned()),
            "id" if !raw_val.contains('\0') => self.id = Some(raw_val.to_owned()),
            _ => {}
        }
        None
    }

    fn flush_event(&mut self) -> Option<SseEventFrame> {
        let event = self.event.take();
        if self.data_lines.is_empty() {
            return None;
        }
        let data = std::mem::take(&mut self.data_lines).join("\n");
        let is_done = data.trim() == "[DONE]";
        Some(SseEventFrame {
            event,
            id: self.id.clone(),
            data,
            is_done,
        })
    }
}

fn decode_sse_line(raw: &[u8]) -> Result<&str, NonAcpParseError> {
    let trimmed = raw.strip_suffix(b"\n").unwrap_or(raw);
    let trimmed = trimmed.strip_suffix(b"\r").unwrap_or(trimmed);
    Ok(std::str::from_utf8(trimmed)?)
}
