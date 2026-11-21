//! Unified diff parsing (`similar`), `SoA` line/token buffers, and Markdown span scanning.
//!
//! Exports:
//! - [`parse_unified_diff`], [`diff_texts_unified`], and [`decode_diff_file_name`] (preserving
//!   leading and trailing ASCII spaces via trailing `[\t\r\n]+` trimming).
//! - [`DiffLineArrays`], [`DiffLineView`], [`DiffTokenInput`], [`DiffTokenView`],
//!   [`FontStyleFlags`], and [`Utf16Span`] in contiguous Structure-of-Arrays (`SoA`) memory.
//! - [`scan_markdown_spans`], [`keep_windows_path_destination`], [`is_windows_absolute_path`],
//!   and [`may_contain_math`] for math- and Windows-path-safe Markdown analysis.
//!
//! # Examples
//!
//! ```
//! use t3_client_core::diff_markdown::{
//!     MarkdownSpanKind, decode_diff_file_name, parse_unified_diff, scan_markdown_spans,
//! };
//!
//! // Trailing spaces in filenames survive header decoding (only `[\t\r\n]+` is stripped).
//! assert_eq!(decode_diff_file_name("b/trail space \t\n", true), "trail space ");
//!
//! let patch = "diff --git \"a/old name \" \"b/new name \"\n\
//! --- \"a/old name \"\n\
//! +++ \"b/new name \"\n\
//! @@ -1 +1 @@\n\
//! -let x = 1;\n\
//! +let x = 2;\n";
//! let files = parse_unified_diff(patch);
//! assert_eq!(files.len(), 1);
//! assert_eq!(files[0].name, "new name ");
//! assert_eq!(files[0].prev_name.as_deref(), Some("old name "));
//! assert_eq!(files[0].lines.line_word_diffs(0).len(), 1);
//!
//! let spans = scan_markdown_spans(r"See C:\me\.t3\shot.png and \(x^2\) or $5 and $10.");
//! assert!(matches!(
//!     &spans[0].kind,
//!     MarkdownSpanKind::WindowsPath { path, in_destination: false } if path == r"C:\me\.t3\shot.png"
//! ));
//! assert!(matches!(
//!     &spans[1].kind,
//!     MarkdownSpanKind::Math { display: false, tex, .. } if tex == "x^2"
//! ));
//! assert_eq!(spans.len(), 2); // `$5 and $10` stays literal prose
//! ```

pub use crate::diff_parser::{
    DiffChangeType, DiffHunkMeta, ParsedDiffFile, decode_diff_file_name, diff_texts_unified,
    parse_unified_diff,
};
pub use crate::diff_soa::{
    DiffLineArrays, DiffLineKind, DiffLineView, DiffTokenInput, DiffTokenView, FontStyleFlags,
    SliceRange, TextRange, Utf16Span,
};
pub use crate::markdown_scanner::{
    MarkdownSpan, MarkdownSpanKind, MathDelimiter, is_unc_path, is_windows_absolute_path,
    is_windows_drive_path, keep_windows_path_destination, may_contain_math, scan_markdown_spans,
};
