//! Tests and `proptest` suites for `diff_markdown` (filename space preservation, `SoA`
//! `DiffLineArrays`, and math/Windows-path-safe Markdown span scanning).

use legion_client_core::diff_markdown::{
    DiffChangeType, DiffLineArrays, DiffLineKind, DiffTokenInput, FontStyleFlags, MarkdownSpanKind,
    MathDelimiter, decode_diff_file_name, diff_texts_unified, keep_windows_path_destination,
    may_contain_math, parse_unified_diff, scan_markdown_spans,
};
use proptest::prelude::*;

#[test]
fn decode_diff_file_name_preserves_leading_and_trailing_spaces() {
    assert_eq!(
        decode_diff_file_name("b/trail space \t\r\n", true),
        "trail space "
    );
    assert_eq!(
        decode_diff_file_name("a/ leading space ", true),
        " leading space "
    );
    assert_eq!(
        decode_diff_file_name("\"a/caf\\303\\251/r\\303\\251sum\\303\\251.ts\"\t\n", true),
        "café/résumé.ts"
    );
    assert_eq!(
        decode_diff_file_name("\"b/new\\tname \"", true),
        "new\tname "
    );
    assert_eq!(
        decode_diff_file_name("a/a/example.ts", true),
        "a/example.ts"
    );
}

#[test]
fn parse_unified_diff_preserves_spaces_renames_eof_and_word_diffs() {
    let patch = concat!(
        "diff --git a/old space  b/new space \n",
        "similarity index 85%\n",
        "rename from old space \t\n",
        "rename to new space  \t\r\n",
        "--- a/old space \t\n",
        "+++ b/new space  \t\n",
        "@@ -1,2 +1,2 @@\n",
        " keep line \n",
        "-let alpha = 1;\n",
        "+let beta = 1;\n",
        "\\ No newline at end of file\n",
    );
    let files = parse_unified_diff(patch);
    assert_eq!(files.len(), 1);
    let file = &files[0];
    assert_eq!(file.name, "new space  ");
    assert_eq!(file.prev_name.as_deref(), Some("old space "));
    assert_eq!(file.change_type, DiffChangeType::RenameChanged);
    assert_eq!(file.additions, 1);
    assert_eq!(file.deletions, 1);
    assert!(file.hunks[0].no_eof_cr_additions);
    assert!(!file.hunks[0].no_eof_cr_deletions);
    assert_eq!(file.lines.line_text(0), Some("keep line "));
    assert_eq!(file.lines.line_text(1), Some("let alpha = 1;"));
    assert_eq!(file.lines.line_text(2), Some("let beta = 1;"));
    assert_eq!(file.lines.line_word_diffs(1).len(), 1);
    assert_eq!(file.lines.line_word_diffs(2).len(), 1);
}

#[test]
fn diff_texts_unified_and_soa_line_arrays_preserve_font_style() {
    let parsed = diff_texts_unified("a/foo .rs", "let a = 1;\n", "b/foo .rs", "let b = 1;\n", 3);
    assert_eq!(parsed.name, "foo .rs");
    assert_eq!(parsed.lines.len(), 2);

    let mut soa = DiffLineArrays::new();
    let idx = soa.push_line(DiffLineKind::Add, 0, None, Some(1), "let 😀 = 1;");
    let style = FontStyleFlags::BOLD | FontStyleFlags::ITALIC | FontStyleFlags::UNDERLINE;
    assert!(soa.set_line_tokens(
        idx,
        &[
            DiffTokenInput {
                byte_offset: 0,
                byte_len: 3,
                color: Some(0x00FF_00AA),
                font_style: style,
            },
            DiffTokenInput {
                byte_offset: 4,
                byte_len: 4,
                color: None,
                font_style: FontStyleFlags::NONE,
            },
        ],
    ));
    let tokens = soa.line_tokens(idx);
    assert_eq!(tokens.len(), 2);
    assert_eq!(tokens[0].text, "let");
    assert_eq!(tokens[0].font_style, style);
    assert_eq!(tokens[1].text, "😀");
}

#[test]
fn markdown_scanner_preserves_windows_paths_and_rejects_false_math() {
    assert_eq!(
        keep_windows_path_destination(r"C:\me\.legioncode\shot.png"),
        r"C:\me\.legioncode\shot.png"
    );
    assert_eq!(
        keep_windows_path_destination(r"docs/a\*b.md"),
        "docs/a*b.md"
    );

    let doc = concat!(
        "It costs $5 and $10 or $HOME,$PATH.\n\n",
        "As shown in \\[1\\] and \\[2, 3\\].\n\n",
        "Link [shot](C:\\me\\.legioncode\\shot.png) and prose D:\\work\\(foo)\\[1].txt.\n\n",
        "Inline $x^2$, \\(y_1\\), display $$a+b$$, and \\[c\\].\n\n",
        "Path \\\\\\(z^2\\) works after escaped backslash.\n\n",
        "```math\n\\sum_i x_i\n```\n\n",
        "```math\nunclosed\n",
    );
    let spans = scan_markdown_spans(doc);
    let win_paths: Vec<(&str, bool)> = spans
        .iter()
        .filter_map(|s| match &s.kind {
            MarkdownSpanKind::WindowsPath {
                path,
                in_destination,
            } => Some((path.as_str(), *in_destination)),
            MarkdownSpanKind::Math { .. } => None,
        })
        .collect();
    assert_eq!(
        win_paths,
        vec![
            (r"C:\me\.legioncode\shot.png", true),
            (r"D:\work\(foo)\[1].txt", false),
        ]
    );

    let maths: Vec<(bool, MathDelimiter, &str)> = spans
        .iter()
        .filter_map(|s| match &s.kind {
            MarkdownSpanKind::Math {
                display,
                delimiter,
                tex,
            } => Some((*display, *delimiter, tex.as_str())),
            MarkdownSpanKind::WindowsPath { .. } => None,
        })
        .collect();
    assert_eq!(
        maths,
        vec![
            (false, MathDelimiter::SingleDollar, "x^2"),
            (false, MathDelimiter::Paren, "y_1"),
            (true, MathDelimiter::DoubleDollar, "a+b"),
            (true, MathDelimiter::Bracket, "c"),
            (false, MathDelimiter::Paren, "z^2"),
            (true, MathDelimiter::MathFence, "\\sum_i x_i"),
        ]
    );
}

#[test]
fn markdown_scanner_handles_tables_code_urls_and_bounded_unclosed_scans() {
    assert!(!may_contain_math("It costs $5 and $10."));
    assert!(may_contain_math("Formula $x$ here."));

    let tricky = concat!(
        "    $not_indented_math$\n\n",
        "Use `$x$` and \\(a`\\)` and https://example.com/?$select=id and $y$.\n\n",
        "| Col | Math |\n",
        "| --- | --- |\n",
        "| a | \\(x |\n",
        "| b | y\\) |\n",
        "| c | $|z|$ |\n\n",
        "$$a$$ trailing text\n",
        "$$\n",
        "b\n",
        "$$\n",
    );
    let spans = scan_markdown_spans(tricky);
    let formulas: Vec<(&str, bool)> = spans
        .iter()
        .filter_map(|s| match &s.kind {
            MarkdownSpanKind::Math { tex, .. } => Some((tex.as_str(), s.in_table)),
            MarkdownSpanKind::WindowsPath { .. } => None,
        })
        .collect();
    assert_eq!(
        formulas,
        vec![("y", false), ("|z|", true), ("a", false), ("b", false)]
    );

    // Linear anti-backtracking on repeated unclosed delimiters.
    let unclosed = "\\( a ".repeat(300);
    assert_eq!(scan_markdown_spans(&unclosed), Vec::new());
}

proptest! {
    #[test]
    fn prop_diff_filename_trailing_spaces_survive_parse(
        stem in "[a-zA-Z0-9_-]{1,12}",
        leading_spaces in 0_usize..4,
        trailing_spaces in 1_usize..5,
    ) {
        let name = format!("{}{}{}", " ".repeat(leading_spaces), stem, " ".repeat(trailing_spaces));
        let raw_header = format!("b/{name}\t\r\n");
        prop_assert_eq!(decode_diff_file_name(&raw_header, true), name.as_str());

        let patch = format!(
            "diff --git \"a/{name}\" \"b/{name}\"\n--- \"a/{name}\"\n+++ \"b/{name}\"\n@@ -1 +1 @@\n-old\n+new\n"
        );
        let parsed = parse_unified_diff(&patch);
        prop_assert_eq!(parsed.len(), 1);
        prop_assert_eq!(parsed[0].name.as_str(), name.as_str());
    }

    #[test]
    fn prop_markdown_scanner_never_panics_and_utf16_offsets_align(
        s in "([a-zA-Z0-9 $()\\[\\]\\\\`:|._\n\r😀]{0,80})",
    ) {
        let spans = scan_markdown_spans(&s);
        let mut prev_end = 0_usize;
        for span in spans {
            prop_assert!(span.byte_start >= prev_end);
            prop_assert!(span.byte_end <= s.len());
            prop_assert!(s.is_char_boundary(span.byte_start));
            prop_assert!(s.is_char_boundary(span.byte_end));
            prop_assert_eq!(span.utf16_start, s[..span.byte_start].encode_utf16().count());
            prop_assert_eq!(span.utf16_end, s[..span.byte_end].encode_utf16().count());
            prev_end = span.byte_end;
        }
    }
}
