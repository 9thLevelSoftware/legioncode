//! Incremental HTTP/1.x proxy `Host:` header rewriter (`BUG-26`).

use serde::{Deserialize, Serialize};

const MAX_HEADER_BUFFER_BYTES: usize = 64 * 1024;

/// Lifecycle phase of [`HttpHeaderHostSwap`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HttpProxyPhase {
    /// Accumulating HTTP/1.x headers until the `\r\n\r\n` separator arrives.
    HeaderPhase,
    /// Permanent pass-through state entered at `\r\n\r\n`; bodies are never scanned or rewritten.
    BodyPassthrough,
}

/// Incremental HTTP/1.x proxy host rewriter (`BUG-26`).
///
/// Buffers only until `\r\n\r\n` (even when `\r\n\r\n` is split across chunk boundaries),
/// rewrites ONLY the `Host:` header line matching `from_host` -> `to_host`, and then
/// transitions permanently to [`HttpProxyPhase::BodyPassthrough`] so JSON or raw payload text
/// containing `"Host: ..."` is never mutated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpHeaderHostSwap {
    from_host: String,
    to_host: String,
    phase: HttpProxyPhase,
    header_buffer: Vec<u8>,
}

impl HttpHeaderHostSwap {
    /// Creates a new incremental HTTP header `Host:` rewriter in [`HttpProxyPhase::HeaderPhase`].
    pub fn new(from_host: impl Into<String>, to_host: impl Into<String>) -> Self {
        Self {
            from_host: from_host.into(),
            to_host: to_host.into(),
            phase: HttpProxyPhase::HeaderPhase,
            header_buffer: Vec::new(),
        }
    }

    /// Current proxy state (`HeaderPhase` or `BodyPassthrough`).
    pub const fn phase(&self) -> HttpProxyPhase {
        self.phase
    }

    /// Source hostname matched case-insensitively inside `Host:` headers.
    pub fn from_host(&self) -> &str {
        &self.from_host
    }

    /// Replacement hostname written into matching `Host:` headers.
    pub fn to_host(&self) -> &str {
        &self.to_host
    }

    /// Feeds one incoming byte chunk into the state machine and returns bytes ready to forward.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<u8> {
        if self.phase == HttpProxyPhase::BodyPassthrough {
            return chunk.to_vec();
        }
        if chunk.is_empty() {
            return Vec::new();
        }

        self.header_buffer.extend_from_slice(chunk);
        if let Some(end_idx) = find_crlf_crlf(&self.header_buffer) {
            let mut out = self.rewrite_header_block(&self.header_buffer[..end_idx]);
            out.extend_from_slice(b"\r\n\r\n");
            out.extend_from_slice(&self.header_buffer[end_idx + 4..]);
            self.header_buffer.clear();
            self.phase = HttpProxyPhase::BodyPassthrough;
            return out;
        }

        if self.header_buffer.len() > MAX_HEADER_BUFFER_BYTES {
            self.phase = HttpProxyPhase::BodyPassthrough;
            return std::mem::take(&mut self.header_buffer);
        }

        Vec::new()
    }

    /// Flushes any incomplete buffered bytes unchanged if the stream closes before `\r\n\r\n`.
    pub fn finish(&mut self) -> Vec<u8> {
        self.phase = HttpProxyPhase::BodyPassthrough;
        std::mem::take(&mut self.header_buffer)
    }

    fn rewrite_header_block(&self, block: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(block.len());
        let mut line_idx = 0_usize;
        let mut start = 0_usize;

        while start <= block.len() {
            let rel_end = block[start..]
                .windows(2)
                .position(|w| w == b"\r\n")
                .unwrap_or(block.len() - start);
            let line = &block[start..start + rel_end];
            if line_idx > 0 {
                out.extend_from_slice(b"\r\n");
            }
            if line_idx == 0 {
                out.extend_from_slice(line);
            } else {
                self.append_rewritten_header_line(line, &mut out);
            }
            if start + rel_end >= block.len() {
                break;
            }
            start += rel_end + 2;
            line_idx += 1;
        }
        out
    }

    fn append_rewritten_header_line(&self, line: &[u8], out: &mut Vec<u8>) {
        if line.len() < 5 || !line[..5].eq_ignore_ascii_case(b"host:") {
            out.extend_from_slice(line);
            return;
        }
        let Ok(raw_value) = std::str::from_utf8(&line[5..]) else {
            out.extend_from_slice(line);
            return;
        };
        let leading_ws_len = raw_value.len() - raw_value.trim_start_matches([' ', '\t']).len();
        let trailing_ws_len = raw_value.len() - raw_value.trim_end_matches([' ', '\t']).len();
        let value_end = raw_value.len().saturating_sub(trailing_ws_len);
        if leading_ws_len > value_end {
            out.extend_from_slice(line);
            return;
        }
        let trimmed = &raw_value[leading_ws_len..value_end];
        if let Some(replaced) = self.rewrite_host_authority(trimmed) {
            out.extend_from_slice(&line[..5]);
            out.extend_from_slice(&raw_value.as_bytes()[..leading_ws_len]);
            out.extend_from_slice(replaced.as_bytes());
            out.extend_from_slice(&raw_value.as_bytes()[value_end..]);
        } else {
            out.extend_from_slice(line);
        }
    }

    fn rewrite_host_authority(&self, authority: &str) -> Option<String> {
        if authority.is_empty() {
            return None;
        }
        if self.from_host.contains(':') && !self.from_host.starts_with('[') {
            return authority
                .eq_ignore_ascii_case(&self.from_host)
                .then(|| self.to_host.clone());
        }
        let (host_part, port_suffix) = split_authority_host_port(authority);
        if host_part.eq_ignore_ascii_case(&self.from_host) {
            Some(format!("{}{port_suffix}", self.to_host))
        } else {
            None
        }
    }
}

fn find_crlf_crlf(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn split_authority_host_port(authority: &str) -> (&str, &str) {
    if authority.starts_with('[') {
        if let Some(close_idx) = authority.find(']') {
            let host = &authority[..=close_idx];
            let rest = &authority[close_idx + 1..];
            return (host, rest);
        }
        return (authority, "");
    }
    if let Some((host, port)) = authority.rsplit_once(':')
        && !port.is_empty()
        && port.bytes().all(|b| b.is_ascii_digit())
    {
        return (host, &authority[host.len()..]);
    }
    (authority, "")
}
