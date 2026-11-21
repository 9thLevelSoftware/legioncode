//! Unified diff parser powered by `similar`, preserving trailing filename spaces.
//!
//! Incorporates two key fixes from the TypeScript codebase:
//! 1. `@pierre/diffs` `decodeDiffFileName` patch (`patches/@pierre__diffs@1.7.0-beta.4.patch`):
//!    strips only trailing `[\t\r\n]+` instead of `.trim()`, so filenames containing leading or
//!    trailing ASCII spaces (`"file with space .ts"`) are preserved verbatim.
//! 2. Git C-escape and octal UTF-8 filename unquoting (`packages/shared/src/gitPatchPath.ts`).

use serde::{Deserialize, Serialize};
use similar::TextDiff;

use crate::diff_hunk::DiffParserState;
use crate::diff_soa::DiffLineArrays;

/// File-level change kind inferred from git diff headers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiffChangeType {
    /// Existing file modified in place.
    Change,
    /// Newly created file (`--- /dev/null` or `new file mode`).
    New,
    /// Deleted file (`+++ /dev/null` or `deleted file mode`).
    Deleted,
    /// Renamed with 100% similarity and no content hunks.
    RenamePure,
    /// Renamed with content edits.
    RenameChanged,
}

/// Metadata for a single parsed unified diff hunk (`@@ ... @@`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffHunkMeta {
    /// 1-based starting line in the old file.
    pub old_start: u32,
    /// Line count in the old file.
    pub old_count: u32,
    /// 1-based starting line in the new file.
    pub new_start: u32,
    /// Line count in the new file.
    pub new_count: u32,
    /// Optional trailing context text after the closing `@@`.
    pub context: Option<String>,
    /// 0-based index of the hunk's first line inside [`ParsedDiffFile::lines`].
    pub line_start_index: usize,
    /// Total lines stored in [`ParsedDiffFile::lines`] for this hunk (excluding header).
    pub line_count: usize,
    /// Added lines count inside this hunk.
    pub addition_lines: usize,
    /// Deleted lines count inside this hunk.
    pub deletion_lines: usize,
    /// Whether the new file lacks a trailing newline at EOF.
    pub no_eof_cr_additions: bool,
    /// Whether the old file lacked a trailing newline at EOF.
    pub no_eof_cr_deletions: bool,
}

/// Parsed file block containing file paths, hunks, and `SoA` [`DiffLineArrays`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedDiffFile {
    /// Current (or new) path decoded with trailing space preservation.
    pub name: String,
    /// Previous path when distinct (e.g. renames) or `None`.
    pub prev_name: Option<String>,
    /// File change classification.
    pub change_type: DiffChangeType,
    /// Total added lines across all hunks.
    pub additions: usize,
    /// Total deleted lines across all hunks.
    pub deletions: usize,
    /// Hunk descriptors in patch order.
    pub hunks: Vec<DiffHunkMeta>,
    /// Contiguous `SoA` line, word-diff, and token buffers.
    pub lines: DiffLineArrays,
}

/// Decodes a diff header filename while preserving leading and trailing ASCII spaces.
///
/// Mirrors `@pierre/diffs`'s patched `decodeDiffFileName`: strips ONLY trailing `[\t\r\n]+`,
/// unquotes Git `"..."` paths (including octal UTF-8 byte escapes like `\303\251` and `\t`),
/// and optionally strips one `"a/"` or `"b/"` prefix.
pub fn decode_diff_file_name(value: &str, strip_git_prefix: bool) -> String {
    let raw_name = value.trim_end_matches(['\t', '\r', '\n']);
    let decoded = if raw_name.starts_with('"') {
        parse_quoted_diff_file_name(raw_name).unwrap_or_else(|| raw_name.to_owned())
    } else {
        raw_name.to_owned()
    };
    if strip_git_prefix {
        if let Some(rest) = decoded.strip_prefix("a/") {
            return rest.to_owned();
        }
        if let Some(rest) = decoded.strip_prefix("b/") {
            return rest.to_owned();
        }
    }
    decoded
}

fn parse_quoted_diff_file_name(raw: &str) -> Option<String> {
    let inner = raw.strip_prefix('"')?.strip_suffix('"')?;
    Some(unescape_git_quoted_body(inner))
}

fn unescape_git_quoted_body(body: &str) -> String {
    if !body.contains('\\') {
        return body.to_owned();
    }
    let bytes = body.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'\\' {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        let Some(&next) = bytes.get(i + 1) else {
            out.push(b'\\');
            break;
        };
        let named = match next {
            b'"' => Some(b'"'),
            b'\\' => Some(b'\\'),
            b'a' => Some(0x07),
            b'b' => Some(0x08),
            b'f' => Some(0x0c),
            b'n' => Some(0x0a),
            b'r' => Some(0x0d),
            b't' => Some(0x09),
            b'v' => Some(0x0b),
            _ => None,
        };
        if let Some(ch) = named {
            out.push(ch);
            i += 2;
            continue;
        }
        if let Some(octal) = bytes.get(i + 1..i + 4)
            && octal.iter().all(|b| (b'0'..=b'7').contains(b))
        {
            let val = ((octal[0] - b'0') << 6) | ((octal[1] - b'0') << 3) | (octal[2] - b'0');
            out.push(val);
            i += 4;
            continue;
        }
        out.push(next);
        i += 2;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Parses a unified or `git diff` patch string into [`ParsedDiffFile`] records.
///
/// Preserves trailing spaces in file names and line contents, tracks no-EOF-newline markers,
/// and computes intra-line UTF-16 word-diff spans using `similar`.
pub fn parse_unified_diff(patch: &str) -> Vec<ParsedDiffFile> {
    let mut builder = DiffParserState::default();
    for raw_line in patch.split_inclusive('\n') {
        let line = raw_line.strip_suffix('\n').unwrap_or(raw_line);
        let line = line.strip_suffix('\r').unwrap_or(line);
        builder.feed_line(line);
    }
    builder.finish()
}

/// Generates and parses a unified diff between `old_text` and `new_text` using `similar`.
pub fn diff_texts_unified(
    old_name: &str,
    old_text: &str,
    new_name: &str,
    new_text: &str,
    context_radius: usize,
) -> ParsedDiffFile {
    let diff = TextDiff::from_lines(old_text, new_text);
    let patch = diff
        .unified_diff()
        .context_radius(context_radius)
        .header(old_name, new_name)
        .to_string();
    let mut parsed = parse_unified_diff(&patch);
    if let Some(file) = parsed.pop() {
        return file;
    }
    ParsedDiffFile {
        name: decode_diff_file_name(new_name, true),
        prev_name: (old_name != new_name).then(|| decode_diff_file_name(old_name, true)),
        change_type: DiffChangeType::Change,
        additions: 0,
        deletions: 0,
        hunks: Vec::new(),
        lines: DiffLineArrays::new(),
    }
}
