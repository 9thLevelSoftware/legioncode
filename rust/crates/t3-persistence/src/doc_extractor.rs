//! Bounded, Pure Safe Rust document text extractor matching
//! `apps/server/src/provider/documentTextWorker.ts` (`extractDocumentText`) and `binMain.ts:11-20`.

use std::fs;
use std::path::Path;

use thiserror::Error;

use crate::doc_extractor_legacy::{extract_html_parts, extract_ipynb_parts};
use crate::doc_extractor_office::{extract_office_or_epub, read_bounded_archive};
use crate::doc_extractor_rtf::extract_rtf_parts;
use crate::doc_extractor_xml::TextPart;

/// Maximum source file byte size (`16 MiB`).
pub const DOC_MAX_INPUT_BYTES: u64 = 16 * 1024 * 1024;
/// Maximum total uncompressed archive XML/HTML byte size (`32 MiB`).
pub const DOC_MAX_EXPANDED_BYTES: u64 = 32 * 1024 * 1024;
/// Maximum extracted character budget before truncation (`1_000_000` UTF-16/Unicode chars).
pub const DOC_MAX_OUTPUT_CHARS: usize = 1_000_000;
/// Maximum parsed XML element nodes per document part (`200_000`).
pub const DOC_MAX_XML_NODES: usize = 200_000;
/// Maximum XML/RTF nesting depth (`128`).
pub const DOC_MAX_DEPTH: usize = 128;
/// Suffix appended whenever output or structural safety caps stop extraction early.
pub const DOC_TRUNCATED_SUFFIX: &str =
    "\n\n[Extracted text truncated at document extraction limits.]";

/// Errors returned by [`extract_document_text`], preserving retryable OS I/O errors (`exit 1`)
/// vs terminal document rejections (`exit 2` per `apps/server/src/binMain.ts:15-20`).
#[derive(Debug, Error)]
pub enum DocExtractError {
    /// Operating-system / filesystem I/O error (retryable: CLI exit `1`).
    #[error("document I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Unsupported document format/extension (CLI exit `2`).
    #[error("unsupported document format '{0}'")]
    Unsupported(String),
    /// Document parsed successfully without hitting limits, but yielded no visible text (CLI exit `2`).
    #[error("no text extracted from document")]
    NoTextExtracted,
    /// Invalid, corrupt, or encrypted ZIP container (CLI exit `2`).
    #[error("invalid archive: {0}")]
    Archive(String),
    /// Malformed XML or markup syntax (CLI exit `2`).
    #[error("invalid XML/markup: {0}")]
    Xml(String),
    /// Hard structural/format limit or invalid document format (CLI exit `2`).
    #[error("document limit or format error: {0}")]
    LimitExceeded(String),
}

impl DocExtractError {
    /// Returns `1` for retryable OS I/O failures and `2` for document rejections (`binMain.ts:15-20`).
    pub const fn exit_code(&self) -> i32 {
        match self {
            Self::Io(_) => 1,
            Self::Unsupported(_)
            | Self::NoTextExtracted
            | Self::Archive(_)
            | Self::Xml(_)
            | Self::LimitExceeded(_) => 2,
        }
    }
}

pub(crate) enum DocInternalErr {
    Io(std::io::Error),
    Archive(String),
    Xml(String),
    InvalidFormat(String),
    Unsupported(String),
    Truncated,
}

impl From<std::io::Error> for DocInternalErr {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

/// Extracts bounded plain text from `path` for `extension` (with or without leading `.`).
///
/// Supported formats: `.docx`, `.xlsx`, `.pptx`, `.odt`, `.ods`, `.odp`, `.epub`, `.ipynb`,
/// and `.rtf`. Standalone `.html`, `.htm`, and `.xhtml` extensions return
/// [`DocExtractError::Unsupported`] (exit code `2`) to match `documentTextWorker.ts`.
pub fn extract_document_text(path: &Path, extension: &str) -> Result<String, DocExtractError> {
    let ext = extension.trim_start_matches('.').to_ascii_lowercase();
    let meta = fs::metadata(path)?;
    if meta.len() > DOC_MAX_INPUT_BYTES {
        return Ok(DOC_TRUNCATED_SUFFIX.to_owned());
    }

    let mut truncated = false;
    let mut parts = Vec::new();
    match extract_parts(path, &ext, &mut parts) {
        Ok(()) => {}
        Err(DocInternalErr::Truncated) => truncated = true,
        Err(other) => return Err(map_internal_err(other)),
    }
    finalize_extracted_parts(&parts, truncated)
}

/// Extracts bounded plain text directly from HTML or XHTML markup bytes (used internally by
/// EPUB chapters and markup-entity unit tests).
pub fn extract_html_fragment_text(bytes: &[u8], is_xhtml: bool) -> Result<String, DocExtractError> {
    let mut truncated = false;
    let mut parts = Vec::new();
    match extract_html_parts(bytes, is_xhtml, &mut parts) {
        Ok(()) => {}
        Err(DocInternalErr::Truncated) => truncated = true,
        Err(other) => return Err(map_internal_err(other)),
    }
    finalize_extracted_parts(&parts, truncated)
}

fn map_internal_err(err: DocInternalErr) -> DocExtractError {
    match err {
        DocInternalErr::Io(io_err) => DocExtractError::Io(io_err),
        DocInternalErr::Unsupported(fmt) => DocExtractError::Unsupported(fmt),
        DocInternalErr::Archive(msg) => DocExtractError::Archive(msg),
        DocInternalErr::Xml(msg) => DocExtractError::Xml(msg),
        DocInternalErr::InvalidFormat(msg) => DocExtractError::LimitExceeded(msg),
        DocInternalErr::Truncated => DocExtractError::NoTextExtracted,
    }
}

fn finalize_extracted_parts(
    parts: &[TextPart],
    mut truncated: bool,
) -> Result<String, DocExtractError> {
    let mut acc = OutputAccumulator::default();
    if let Err(DocInternalErr::Truncated) = acc.consume_all(parts) {
        truncated = true;
    }

    let cleaned = collapse_blank_lines(&acc.result);
    let trimmed = cleaned.trim();
    if trimmed.is_empty() && !truncated {
        return Err(DocExtractError::NoTextExtracted);
    }
    if truncated {
        Ok(format!("{}{DOC_TRUNCATED_SUFFIX}", trimmed.trim_end()))
    } else {
        Ok(trimmed.to_owned())
    }
}

fn extract_parts(path: &Path, ext: &str, parts: &mut Vec<TextPart>) -> Result<(), DocInternalErr> {
    match ext {
        "ipynb" => {
            let raw = fs::read_to_string(path)?;
            extract_ipynb_parts(&raw, parts)
        }
        "rtf" => {
            let bytes = fs::read(path)?;
            extract_rtf_parts(&bytes, parts)
        }
        "docx" | "xlsx" | "pptx" | "odt" | "ods" | "odp" | "epub" => {
            let files = read_bounded_archive(path)?;
            extract_office_or_epub(&files, ext, parts)
        }
        _ => Err(DocInternalErr::Unsupported(ext.to_owned())),
    }
}

struct OutputAccumulator {
    result: String,
    remaining_utf16: usize,
    cell_depth: usize,
    started: bool,
    space: bool,
}

impl Default for OutputAccumulator {
    fn default() -> Self {
        Self {
            result: String::new(),
            remaining_utf16: DOC_MAX_OUTPUT_CHARS,
            cell_depth: 0,
            started: false,
            space: false,
        }
    }
}

impl OutputAccumulator {
    fn consume_all(&mut self, parts: &[TextPart]) -> Result<(), DocInternalErr> {
        for part in parts {
            match part {
                TextPart::CellStart => {
                    if self.cell_depth == 0 {
                        self.started = false;
                        self.space = false;
                    } else if self.started {
                        self.space = true;
                    }
                    self.cell_depth += 1;
                }
                TextPart::CellEnd => {
                    self.cell_depth = self.cell_depth.saturating_sub(1);
                    if self.cell_depth > 0 && self.started {
                        self.space = true;
                    }
                }
                TextPart::Fallback(raw) => {
                    if !self.started {
                        self.push_text(raw)?;
                    }
                }
                TextPart::Text(raw) => self.push_text(raw)?,
            }
        }
        Ok(())
    }

    fn push_text(&mut self, raw: &str) -> Result<(), DocInternalErr> {
        let mut taken_u16 = 0usize;
        let mut byte_end = 0usize;
        for ch in raw.chars() {
            let u16_len = ch.len_utf16();
            if taken_u16 + u16_len > self.remaining_utf16 {
                break;
            }
            taken_u16 += u16_len;
            byte_end += ch.len_utf8();
        }
        let hit_budget = byte_end < raw.len() || self.remaining_utf16 == taken_u16;
        self.remaining_utf16 = self.remaining_utf16.saturating_sub(taken_u16);
        let value = raw[..byte_end].replace('\0', "");

        if self.cell_depth > 0 {
            let normalized = collapse_whitespace_in_cell(&value);
            let trimmed = normalized.trim();
            if !trimmed.is_empty() {
                let starts_ws = normalized.starts_with(' ');
                if self.started && (self.space || starts_ws) {
                    self.result.push(' ');
                }
                self.result.push_str(trimmed);
                self.started = true;
            }
            self.space = normalized.ends_with(' ') || (normalized.is_empty() && self.space);
        } else {
            self.result.push_str(&value);
        }

        if hit_budget && (!raw.is_empty() || self.remaining_utf16 == 0) {
            return Err(DocInternalErr::Truncated);
        }
        Ok(())
    }
}

fn collapse_whitespace_in_cell(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_ws = false;
    for ch in input.chars() {
        if ch.is_whitespace() {
            if !in_ws {
                out.push(' ');
                in_ws = true;
            }
        } else {
            in_ws = false;
            out.push(ch);
        }
    }
    out
}

fn collapse_blank_lines(input: &str) -> String {
    let lines: Vec<&str> = input.split('\n').collect();
    let mut out: Vec<&str> = Vec::with_capacity(lines.len());
    let mut blank_run = 0usize;
    for line in lines {
        if line.chars().all(|c| c == ' ' || c == '\t') {
            blank_run += 1;
            if blank_run <= 1 {
                out.push("");
            }
        } else {
            blank_run = 0;
            out.push(line);
        }
    }
    out.join("\n")
}
