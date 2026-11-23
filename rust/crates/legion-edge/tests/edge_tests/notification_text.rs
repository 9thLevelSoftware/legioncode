use legion_edge::notification_text::{
    MAX_NOTIFICATION_SUMMARY_UTF16_LEN, truncate_notification_text, truncate_notification_title,
};

#[test]
fn truncates_surrogate_pair_emojis_without_cutting_in_half() {
    let repeated = "😀".repeat(120);
    let truncated = truncate_notification_title(&repeated);

    assert_eq!(truncated, format!("{}...", "😀".repeat(58)));
    assert!(
        truncated.encode_utf16().count() <= MAX_NOTIFICATION_SUMMARY_UTF16_LEN,
        "truncated UTF-16 length should fit limit"
    );
}

#[test]
fn preserves_short_strings_trims_edges_and_keeps_zwj_and_flag_clusters_intact() {
    assert_eq!(
        truncate_notification_title("  Build passed  "),
        "Build passed"
    );
    assert_eq!(truncate_notification_text("hello", 0), "");

    let zwj_title = format!("{}done", "👨‍👩‍👧‍👦".repeat(12));
    let truncated = truncate_notification_text(&zwj_title, 30);
    assert!(truncated.ends_with("..."));
    assert!(truncated.encode_utf16().count() <= 30);
    assert!(!truncated.contains('\u{200D}') || truncated.starts_with(&"👨‍👩‍👧‍👦".repeat(2)));

    let flags = "🇺🇸".repeat(4);
    let flag_truncated = truncate_notification_text(&flags, 10);
    assert_eq!(flag_truncated, "🇺🇸...");
}
