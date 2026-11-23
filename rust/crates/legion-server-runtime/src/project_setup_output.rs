//! UTF-16 surrogate-safe setup script terminal line buffering (`#40701`).

use legion_shared::text_norm::{truncate_prefix_utf16, truncate_tail_utf16};

/// Maximum UTF-16 code units kept per emitted setup output line (`#40701`).
pub const OUTPUT_LINE_MAX_LENGTH: usize = 400;

/// Maximum UTF-16 code units retained in the incomplete trailing line buffer (`#40701`).
pub const PARTIAL_LINE_MAX_LENGTH: usize = 4_096;

/// Strips ANSI CSI/OSC escape sequences and ASCII control characters from a single setup line.
#[must_use]
pub fn strip_terminal_control(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            match chars.peek().copied() {
                Some('[') => {
                    let _ = chars.next();
                    for c in chars.by_ref() {
                        if ('\x40'..='\x7e').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    let _ = chars.next();
                    while let Some(c) = chars.next() {
                        if c == '\x07' {
                            break;
                        }
                        if c == '\x1b' && chars.peek().copied() == Some('\\') {
                            let _ = chars.next();
                            break;
                        }
                    }
                }
                Some(_) => {
                    let _ = chars.next();
                }
                None => {}
            }
            continue;
        }
        if ch.is_control() && ch != '\t' {
            continue;
        }
        out.push(ch);
    }
    out
}

/// Incrementally buffers setup terminal chunks, caps lines without splitting UTF-16 surrogate
/// pairs, and filters empty/sentinel/wrapper echo lines (`#40701`).
#[derive(Debug, Default, Clone)]
pub struct SetupScriptLineBuffer {
    line_buffer: String,
    sentinel: Option<String>,
    echoed_wrapper_lines: Vec<String>,
}

impl SetupScriptLineBuffer {
    /// Creates a new line buffer filtering optional `sentinel` and `echoed_wrapper_lines`.
    #[must_use]
    pub const fn new(sentinel: Option<String>, echoed_wrapper_lines: Vec<String>) -> Self {
        Self {
            line_buffer: String::new(),
            sentinel,
            echoed_wrapper_lines,
        }
    }

    /// Appends a raw PTY output chunk, caps any partial tail on valid UTF-16 boundaries, and
    /// returns cleaned complete lines capped to [`OUTPUT_LINE_MAX_LENGTH`] UTF-16 code units.
    pub fn push_chunk(&mut self, chunk: &str) -> Vec<String> {
        self.line_buffer.push_str(chunk);
        let mut emitted = Vec::new();
        while let Some(newline_pos) = self.line_buffer.find('\n') {
            let raw_line: String = self.line_buffer.drain(..=newline_pos).collect();
            if let Some(cleaned) = self.format_line(&raw_line) {
                emitted.push(cleaned);
            }
        }
        if self.line_buffer.encode_utf16().count() > PARTIAL_LINE_MAX_LENGTH {
            let trimmed = truncate_tail_utf16(&self.line_buffer, PARTIAL_LINE_MAX_LENGTH);
            self.line_buffer = trimmed.to_owned();
        }
        emitted
    }

    /// Flushes any remaining buffered text when the setup shell exits.
    pub fn flush_tail(&mut self) -> Option<String> {
        let raw = std::mem::take(&mut self.line_buffer);
        self.format_line(&raw)
    }

    fn format_line(&self, raw_line: &str) -> Option<String> {
        let stripped = strip_terminal_control(raw_line);
        let cleaned = stripped.trim_end();
        if cleaned.is_empty() {
            return None;
        }
        if let Some(sentinel) = &self.sentinel
            && !sentinel.is_empty()
            && cleaned.contains(sentinel)
        {
            return None;
        }
        if self
            .echoed_wrapper_lines
            .iter()
            .any(|echoed| !echoed.is_empty() && cleaned.ends_with(echoed))
        {
            return None;
        }
        Some(truncate_prefix_utf16(cleaned, OUTPUT_LINE_MAX_LENGTH).to_owned())
    }
}
