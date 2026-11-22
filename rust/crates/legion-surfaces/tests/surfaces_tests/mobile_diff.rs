//! BUG-22 integration tests: unified attributed diff runs preserve font style flags and
//! arbitrary non-color attributes across word-diff splitting and `SoA` lowering.

use std::collections::BTreeMap;

use legion_client_core::diff_markdown::{DiffLineArrays, DiffLineKind, DiffTokenInput, Utf16Span};
use legion_surfaces::mobile_diff::{
    AttributedDiffRun, FontStyleFlags, SyntaxTokenInput, build_syntax_runs_from_tokens,
    decorate_diff_runs, decorate_diff_runs_with_background, decorate_soa_line,
};

fn collect_text(runs: &[AttributedDiffRun]) -> String {
    let mut out = String::new();
    for r in runs {
        out.push_str(&r.text);
    }
    out
}

#[test]
fn font_style_flags_support_all_four_attributes_and_truncation() {
    let all = FontStyleFlags::ITALIC
        | FontStyleFlags::BOLD
        | FontStyleFlags::UNDERLINE
        | FontStyleFlags::STRIKETHROUGH;
    assert_eq!(all.bits(), 15);
    assert!(all.is_italic());
    assert!(all.is_bold());
    assert!(all.is_underline());
    assert!(all.is_strikethrough());

    let masked = FontStyleFlags::from_bits_truncate(0xFF);
    assert_eq!(masked, all);
    assert!(!(FontStyleFlags::BOLD & FontStyleFlags::ITALIC).is_bold());
    assert!(FontStyleFlags::NONE.is_empty());
}

#[test]
fn decorate_preserves_bold_italic_underline_strikethrough_and_non_color_attrs() {
    let mut attrs = BTreeMap::new();
    attrs.insert("customLink".to_owned(), "https://example.test".to_owned());
    attrs.insert("semanticRole".to_owned(), "keyword".to_owned());

    let bold_italic = FontStyleFlags::BOLD | FontStyleFlags::ITALIC;
    let tokens = [
        SyntaxTokenInput {
            content: "pub fn ".to_owned(),
            color: Some("#ff00aa".to_owned()),
            font_style: bold_italic,
            ligatures_enabled: false,
            attributes: attrs.clone(),
        },
        SyntaxTokenInput {
            content: "deprecated_fn()".to_owned(),
            color: Some("#8899aa".to_owned()),
            font_style: FontStyleFlags::UNDERLINE | FontStyleFlags::STRIKETHROUGH,
            ligatures_enabled: true,
            attributes: BTreeMap::new(),
        },
    ];
    let syntax_runs = build_syntax_runs_from_tokens(&tokens);
    // Word highlight starts inside token 0 ("fn") and spans into token 1 ("deprec"):
    // UTF-16: "pub fn " = [0, 7), highlight [4, 13) -> "fn " in tok0 + "deprec" in tok1.
    let result = decorate_diff_runs(&syntax_runs, &[Utf16Span { start: 4, end: 13 }]);

    assert_eq!(collect_text(&result), "pub fn deprecated_fn()");
    assert_eq!(result.len(), 4);

    assert_eq!(result[0].text, "pub ");
    assert!(!result[0].word_highlighted);
    assert_eq!(result[0].font_style, bold_italic);
    assert_eq!(result[0].foreground_color.as_deref(), Some("#ff00aa"));
    assert!(!result[0].ligatures_enabled);
    assert_eq!(result[0].attributes, attrs);

    assert_eq!(result[1].text, "fn ");
    assert!(result[1].word_highlighted);
    assert_eq!(result[1].background_color, None);
    assert_eq!(result[1].font_style, bold_italic);
    assert!(!result[1].ligatures_enabled);
    assert_eq!(result[1].attributes, attrs);

    let bg_result = decorate_diff_runs_with_background(
        &syntax_runs,
        &[Utf16Span { start: 4, end: 13 }],
        "#334455",
    );
    assert_eq!(bg_result[1].background_color.as_deref(), Some("#334455"));
    assert_eq!(bg_result[1].font_style, bold_italic);
    assert_eq!(bg_result[1].attributes, attrs);
    assert_eq!(bg_result[0].background_color, None);

    assert_eq!(result[2].text, "deprec");
    assert!(result[2].word_highlighted);
    assert_eq!(
        result[2].font_style,
        FontStyleFlags::UNDERLINE | FontStyleFlags::STRIKETHROUGH
    );
    assert!(result[2].ligatures_enabled);

    assert_eq!(result[3].text, "ated_fn()");
    assert!(!result[3].word_highlighted);
    assert!(result[3].background_color.is_none());
    assert_eq!(
        result[3].font_style,
        FontStyleFlags::UNDERLINE | FontStyleFlags::STRIKETHROUGH
    );
}

#[test]
fn overlapping_and_unsorted_highlights_merge_without_duplication() {
    let syntax_runs = vec![
        AttributedDiffRun::new(0, 10, "abcdefghij")
            .with_foreground_color("#123456")
            .with_font_style(FontStyleFlags::ITALIC),
    ];
    let spans = [
        Utf16Span { start: 6, end: 9 },
        Utf16Span { start: 2, end: 5 },
        Utf16Span { start: 4, end: 7 },
        Utf16Span { start: 8, end: 8 },
    ];
    let out = decorate_diff_runs_with_background(&syntax_runs, &spans, "#442211");
    assert_eq!(out.len(), 3);
    assert_eq!(out[0].text, "ab");
    assert!(!out[0].word_highlighted);
    assert_eq!(out[1].text, "cdefghi");
    assert!(out[1].word_highlighted);
    assert_eq!(out[1].background_color.as_deref(), Some("#442211"));
    assert_eq!(out[1].font_style, FontStyleFlags::ITALIC);
    assert_eq!(out[2].text, "j");
    assert!(!out[2].word_highlighted);
}

#[test]
fn utf16_surrogate_and_cjk_boundaries_split_accurately() {
    // "👋" = 2 UTF-16 code units; "中" = 1 UTF-16 code unit; "!" = 1 UTF-16 code unit.
    let line = "👋中!";
    let run = AttributedDiffRun::new(0, 4, line)
        .with_font_style(FontStyleFlags::BOLD)
        .with_ligatures(false)
        .with_note(true);
    let out = decorate_diff_runs(&[run], &[Utf16Span { start: 2, end: 3 }]);
    assert_eq!(out.len(), 3);
    assert_eq!(out[0].text, "👋");
    assert_eq!((out[0].start, out[0].end), (0, 2));
    assert!(!out[0].word_highlighted);
    assert_eq!(out[1].text, "中");
    assert_eq!((out[1].start, out[1].end), (2, 3));
    assert!(out[1].word_highlighted);
    assert!(out[1].is_note);
    assert_eq!(out[1].font_style, FontStyleFlags::BOLD);
    assert!(!out[1].ligatures_enabled);
    assert_eq!(out[2].text, "!");
    assert_eq!((out[2].start, out[2].end), (3, 4));
    assert!(!out[2].word_highlighted);
}

#[test]
fn identical_adjacent_syntax_runs_coalesce() {
    let r1 = AttributedDiffRun::new(0, 3, "foo")
        .with_foreground_color("#aabbcc")
        .with_font_style(FontStyleFlags::BOLD);
    let r2 = AttributedDiffRun::new(3, 6, "bar")
        .with_foreground_color("#aabbcc")
        .with_font_style(FontStyleFlags::BOLD);
    let out = decorate_diff_runs(&[r1, r2], &[]);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].text, "foobar");
    assert_eq!((out[0].start, out[0].end), (0, 6));
    assert_eq!(out[0].font_style, FontStyleFlags::BOLD);
}

#[test]
fn decorate_soa_line_preserves_font_styles_for_adds_and_skips_word_diffs_on_context()
-> Result<(), String> {
    let mut arrays = DiffLineArrays::new();
    let add_idx = arrays.push_line(DiffLineKind::Add, 0, None, Some(10), "let x = 42;");
    arrays.set_line_tokens(
        add_idx,
        &[
            DiffTokenInput {
                byte_offset: 0,
                byte_len: 4,
                color: Some(0x00FF_00AA),
                font_style: 2, // BOLD
            },
            DiffTokenInput {
                byte_offset: 4,
                byte_len: 7,
                color: Some(0x00AA_BBCC),
                font_style: 1, // ITALIC
            },
        ],
    );
    arrays.set_line_word_diffs(add_idx, &[Utf16Span { start: 8, end: 10 }]);

    let add_runs = decorate_soa_line(&arrays, add_idx).ok_or("missing add line")?;
    assert_eq!(collect_text(&add_runs), "let x = 42;");
    assert_eq!(add_runs[0].font_style, FontStyleFlags::BOLD);
    assert_eq!(add_runs[0].foreground_color.as_deref(), Some("#ff00aa"));
    let highlighted: Vec<&AttributedDiffRun> =
        add_runs.iter().filter(|r| r.word_highlighted).collect();
    assert_eq!(highlighted.len(), 1);
    assert_eq!(highlighted[0].text, "42");
    assert_eq!(highlighted[0].font_style, FontStyleFlags::ITALIC);

    let ctx_idx = arrays.push_line(DiffLineKind::Context, 0, Some(1), Some(1), "unchanged");
    arrays.set_line_word_diffs(ctx_idx, &[Utf16Span { start: 0, end: 3 }]);
    let ctx_runs = decorate_soa_line(&arrays, ctx_idx).ok_or("missing ctx line")?;
    assert!(ctx_runs.iter().all(|r| !r.word_highlighted));
    assert!(decorate_soa_line(&arrays, 99).is_none());
    Ok(())
}
