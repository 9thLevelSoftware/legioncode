//! `SHA-256` file content-fingerprint tracking and git patch path quoting helpers.
//!
//! Backs diff viewed-file invalidation (`DiffFileRevisionTracker`), `cutLineNote`
//! formatting, and reversible C-style git patch filename quoting without altering
//! intentional filename whitespace.

use std::collections::HashMap;

use sha2::{Digest, Sha256};

use crate::core::CoreError;

/// Fingerprint recorded for a single file revision in a diff set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffFileFingerprint {
    /// Repository-relative file path.
    pub path: String,
    /// Lowercase hex `SHA-256` digest of raw file bytes.
    pub sha256: String,
    /// Content size in bytes.
    pub byte_len: usize,
    /// Number of logical lines in the content.
    pub line_count: usize,
    /// Optional truncation line offset when a file preview was cut short.
    pub cut_line: Option<usize>,
}

/// Comparison result between an expected viewed revision and the tracker's current file digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RevisionMatch {
    /// Tracked file revision still matches `expected_sha256`.
    Current,
    /// File changed since it was last viewed.
    Stale {
        /// Latest tracked `SHA-256` hex digest.
        actual_sha256: String,
    },
    /// Path is not present in the tracker.
    Missing,
}

/// Tracks current `SHA-256` file fingerprints alongside user-viewed digests.
///
/// ```
/// use legion_scm::core::revisions::{DiffFileRevisionTracker, RevisionMatch};
///
/// let mut tracker = DiffFileRevisionTracker::new();
/// tracker.record_file("src/lib.rs", b"pub fn a() {}\n", None);
/// let digest = tracker.mark_viewed("src/lib.rs")?;
/// assert!(tracker.is_viewed_at_current_revision("src/lib.rs"));
///
/// tracker.record_file("src/lib.rs", b"pub fn a() { 1; }\n", None);
/// assert!(!tracker.is_viewed_at_current_revision("src/lib.rs"));
/// assert!(matches!(
///     tracker.compare_revision("src/lib.rs", &digest),
///     RevisionMatch::Stale { .. }
/// ));
/// assert_eq!(tracker.stale_viewed_files(), vec!["src/lib.rs"]);
/// # Ok::<(), legion_scm::core::CoreError>(())
/// ```
#[derive(Debug, Default, Clone)]
pub struct DiffFileRevisionTracker {
    current: HashMap<String, DiffFileFingerprint>,
    viewed: HashMap<String, String>,
}

impl DiffFileRevisionTracker {
    /// Creates an empty revision tracker.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Computes and stores the `SHA-256` fingerprint for `path`.
    pub fn record_file(
        &mut self,
        path: &str,
        content: &[u8],
        cut_line: Option<usize>,
    ) -> &DiffFileFingerprint {
        let sha256 = sha256_hex(content);
        let line_count = count_lines(content);
        let fp = DiffFileFingerprint {
            path: path.to_owned(),
            sha256,
            byte_len: content.len(),
            line_count,
            cut_line,
        };
        match self.current.entry(path.to_owned()) {
            std::collections::hash_map::Entry::Occupied(mut occupied) => {
                occupied.insert(fp);
                occupied.into_mut()
            }
            std::collections::hash_map::Entry::Vacant(vacant) => vacant.insert(fp),
        }
    }

    /// Marks `path` as viewed at its current tracked `SHA-256` digest and returns that digest.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::InvalidRevision`] if `path` has not been recorded yet.
    pub fn mark_viewed(&mut self, path: &str) -> Result<String, CoreError> {
        let fp = self
            .current
            .get(path)
            .ok_or_else(|| CoreError::InvalidRevision(format!("untracked path: {path}")))?;
        let digest = fp.sha256.clone();
        self.viewed.insert(path.to_owned(), digest.clone());
        Ok(digest)
    }

    /// Clears the viewed status of `path`.
    pub fn unmark_viewed(&mut self, path: &str) -> bool {
        self.viewed.remove(path).is_some()
    }

    /// Returns `true` when `path` is marked viewed and its `SHA-256` still matches current content.
    #[must_use]
    pub fn is_viewed_at_current_revision(&self, path: &str) -> bool {
        let Some(viewed_sha) = self.viewed.get(path) else {
            return false;
        };
        self.current
            .get(path)
            .is_some_and(|fp| &fp.sha256 == viewed_sha)
    }

    /// Compares a stored `expected_sha256` digest against the tracker's latest file fingerprint.
    #[must_use]
    pub fn compare_revision(&self, path: &str, expected_sha256: &str) -> RevisionMatch {
        match self.current.get(path) {
            Some(fp) if fp.sha256 == expected_sha256 => RevisionMatch::Current,
            Some(fp) => RevisionMatch::Stale {
                actual_sha256: fp.sha256.clone(),
            },
            None => RevisionMatch::Missing,
        }
    }

    /// Returns sorted paths that were previously marked viewed but changed in later revisions.
    #[must_use]
    pub fn stale_viewed_files(&self) -> Vec<String> {
        let mut stale: Vec<String> = self
            .viewed
            .iter()
            .filter_map(|(path, viewed_sha)| {
                self.current
                    .get(path)
                    .filter(|fp| &fp.sha256 != viewed_sha)
                    .map(|_| path.clone())
            })
            .collect();
        stale.sort();
        stale
    }

    /// Returns the recorded fingerprint for `path`, if tracked.
    #[must_use]
    pub fn get(&self, path: &str) -> Option<&DiffFileFingerprint> {
        self.current.get(path)
    }
}

/// Computes the lowercase hex `SHA-256` digest of `bytes`.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Formats a human-readable truncation annotation when a diff file was cut at `cut_line`.
#[must_use]
pub fn cut_line_note(cut_line: Option<usize>) -> Option<String> {
    cut_line.map(|line| format!("Diff truncated at line {line}"))
}

/// Quotes a git patch path with C-style escapes only when needed, preserving ordinary ASCII spaces.
#[must_use]
pub fn quote_git_patch_path(path: &str) -> String {
    let needs_quote = path
        .bytes()
        .any(|b| b < 0x20 || b == 0x7f || b == b'"' || b == b'\\' || !b.is_ascii());
    if !needs_quote {
        return path.to_owned();
    }

    let mut out = String::with_capacity(path.len() + 4);
    out.push('"');
    for byte in path.as_bytes() {
        match *byte {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            b'\x07' => out.push_str("\\a"),
            b'\x08' => out.push_str("\\b"),
            b'\t' => out.push_str("\\t"),
            b'\n' => out.push_str("\\n"),
            b'\x0b' => out.push_str("\\v"),
            b'\x0c' => out.push_str("\\f"),
            b'\r' => out.push_str("\\r"),
            b if !(0x20..0x7f).contains(&b) => {
                out.push('\\');
                out.push(char::from(b'0' + (b >> 6)));
                out.push(char::from(b'0' + ((b >> 3) & 7)));
                out.push(char::from(b'0' + (b & 7)));
            }
            b => out.push(char::from(b)),
        }
    }
    out.push('"');
    out
}

/// Unquotes a C-escaped git patch path (or returns unquoted input verbatim without trimming spaces).
///
/// # Errors
///
/// Returns [`CoreError::InvalidRevision`] if quoted escape sequences or `UTF-8` bytes are invalid.
pub fn unquote_git_patch_path(raw: &str) -> Result<String, CoreError> {
    if !(raw.starts_with('"') && raw.ends_with('"') && raw.len() >= 2) {
        return Ok(raw.to_owned());
    }
    let inner = &raw[1..raw.len() - 1];
    let bytes = inner.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'\\' {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        i += 1;
        if i >= bytes.len() {
            return Err(CoreError::InvalidRevision(
                "dangling backslash in quoted git path".to_owned(),
            ));
        }
        match bytes[i] {
            b'"' => {
                out.push(b'"');
                i += 1;
            }
            b'\\' => {
                out.push(b'\\');
                i += 1;
            }
            b'a' => {
                out.push(b'\x07');
                i += 1;
            }
            b'b' => {
                out.push(b'\x08');
                i += 1;
            }
            b't' => {
                out.push(b'\t');
                i += 1;
            }
            b'n' => {
                out.push(b'\n');
                i += 1;
            }
            b'v' => {
                out.push(b'\x0b');
                i += 1;
            }
            b'f' => {
                out.push(b'\x0c');
                i += 1;
            }
            b'r' => {
                out.push(b'\r');
                i += 1;
            }
            d0 @ b'0'..=b'3' => {
                if i + 2 >= bytes.len()
                    || !matches!(bytes[i + 1], b'0'..=b'7')
                    || !matches!(bytes[i + 2], b'0'..=b'7')
                {
                    return Err(CoreError::InvalidRevision(
                        "invalid octal escape in git path".to_owned(),
                    ));
                }
                let val = (d0 - b'0') * 64 + (bytes[i + 1] - b'0') * 8 + (bytes[i + 2] - b'0');
                out.push(val);
                i += 3;
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
    String::from_utf8(out)
        .map_err(|e| CoreError::InvalidRevision(format!("non-UTF-8 unquoted path: {e}")))
}

fn count_lines(bytes: &[u8]) -> usize {
    if bytes.is_empty() {
        return 0;
    }
    let segments = bytes.split(|&b| b == b'\n').count();
    segments - usize::from(bytes.last() == Some(&b'\n'))
}
