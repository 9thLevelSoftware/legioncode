//! Tests for `oauth_callback`, `text_norm`, `url_authority::is_loopback_host`, and branded desktop control.

use legion_contracts::brand::LEGION_CODE;
use legion_shared::desktop_app_control::{
    DesktopControlPlatform, resolve_desktop_app_control_address_for_brand,
};
use legion_shared::oauth_callback::{
    MAX_OAUTH_CALLBACK_LEN_UTF16, OAuthCallbackError, oauth_callback_url, pasted_url,
    validate_codex_callback_url,
};
use legion_shared::text_norm::{
    accent_folded_contains, count_unique_pending_subagent_threads, dedup_pending_subagent_threads,
    file_display_name, fold_accents, normalize_nfc, splits_surrogate_pair_at,
    truncate_prefix_chars, truncate_prefix_graphemes, truncate_prefix_utf16, truncate_tail_utf16,
};
use legion_shared::url_authority::is_loopback_host;

#[test]
fn oauth_callback_validation_matches_ts_contracts() {
    let redirect = "http://localhost:1455/auth/callback";
    let ok = oauth_callback_url(
        "localhost:1455/auth/callback?state=st-1&code=code-99",
        redirect,
        "st-1",
    )
    .expect("valid schemeless pasted url");
    assert_eq!(ok.origin, "http://localhost:1455");
    assert_eq!(ok.pathname, "/auth/callback");
    assert_eq!(ok.code.as_deref(), Some("code-99"));
    assert_eq!(ok.error, None);

    let default_port = oauth_callback_url(
        "http://127.0.0.1:80/cb?state=s&error=access_denied&error_description=No",
        "http://127.0.0.1/cb",
        "s",
    )
    .expect("port 80 normalizes to default origin");
    assert_eq!(default_port.error.as_deref(), Some("access_denied"));
    assert_eq!(default_port.error_description.as_deref(), Some("No"));

    let empty_hash = oauth_callback_url(
        "http://localhost:1455/auth/callback?state=st-1&code=c#",
        redirect,
        "st-1",
    );
    assert!(empty_hash.is_ok());

    assert_eq!(
        oauth_callback_url(
            "http://evil.test:1455/auth/callback?state=st-1&code=c",
            redirect,
            "st-1"
        ),
        Err(OAuthCallbackError::RedirectMismatch)
    );
    assert_eq!(
        oauth_callback_url(
            "http://user@localhost:1455/auth/callback?state=st-1&code=c",
            redirect,
            "st-1"
        ),
        Err(OAuthCallbackError::RedirectMismatch)
    );
    assert_eq!(
        oauth_callback_url(
            "http://localhost:1455/auth/callback?state=st-1&code=c#frag",
            redirect,
            "st-1"
        ),
        Err(OAuthCallbackError::RedirectMismatch)
    );
    assert_eq!(
        oauth_callback_url(
            "http://localhost:1455/auth/callback?state=st-1&code=c&error=e",
            redirect,
            "st-1"
        ),
        Err(OAuthCallbackError::RedirectMismatch)
    );
    assert_eq!(
        oauth_callback_url(
            "http://localhost:1455/auth/callback?state=st-1&state=st-1&code=c",
            redirect,
            "st-1",
        ),
        Err(OAuthCallbackError::RedirectMismatch)
    );

    let long_url = format!(
        "http://localhost:1455/auth/callback?state=st-1&code={}",
        "x".repeat(MAX_OAUTH_CALLBACK_LEN_UTF16)
    );
    assert_eq!(
        oauth_callback_url(&long_url, redirect, "st-1"),
        Err(OAuthCallbackError::RedirectMismatch)
    );

    let codex_ok = validate_codex_callback_url(
        "http://localhost:1455/auth/callback?state=s&code=c&client_id=oaiapp_abc-1",
        redirect,
        "s",
    );
    assert!(codex_ok.is_ok());
    assert_eq!(
        validate_codex_callback_url(
            "http://localhost:1455/auth/callback?state=s&code=c&client_id=bad_client",
            redirect,
            "s",
        ),
        Err(OAuthCallbackError::RedirectMismatch)
    );
    assert!(pasted_url("   ", None).is_err());
}

#[test]
fn text_norm_composes_nfd_folds_accents_truncates_safely_and_dedups_subagents() {
    // macOS NFD filenames & multi-mark Vietnamese & Hangul Jamo composition
    assert_eq!(normalize_nfc("re\u{0301}sume\u{0301}.txt"), "résumé.txt");
    assert_eq!(normalize_nfc("Vie\u{0302}\u{0323}t.md"), "Việt.md");
    assert_eq!(normalize_nfc("Vie\u{0323}\u{0302}t.md"), "Việt.md");
    assert_eq!(normalize_nfc("\u{1112}\u{1161}\u{11AB}.txt"), "한.txt");
    assert_eq!(
        file_display_name("/Users/alice/Desktop/Cafe\u{0301} notes/"),
        "Café notes"
    );

    // Accent folding for thread-link search
    assert_eq!(fold_accents("Café planning"), "cafe planning");
    assert_eq!(fold_accents("Cafe\u{0301} planning"), "cafe planning");
    assert!(accent_folded_contains("Café planning", "cafe"));
    assert!(accent_folded_contains("naïve résumé", "naive resume"));
    assert!(!accent_folded_contains("Café", "tea"));

    // UTF-16 & grapheme truncation never splits emoji / surrogate pairs / ZWJ sequences
    let s = format!("API Error: 400 {}\u{1F680} rejected", "x".repeat(484));
    assert_eq!(
        truncate_prefix_utf16(&s, 500),
        format!("API Error: 400 {}", "x".repeat(484))
    );
    assert!(splits_surrogate_pair_at("a\u{1F680}b", 2));
    assert!(!splits_surrogate_pair_at("a\u{1F680}b", 1));
    assert!(!splits_surrogate_pair_at("a\u{1F680}b", 3));

    assert_eq!(truncate_tail_utf16("prefix\r\n\u{1F680}ok", 3), "ok");
    assert_eq!(truncate_prefix_chars("héllo🚀", 5), "héllo");
    assert_eq!(
        truncate_prefix_graphemes("e\u{0301}👨‍👩‍👧‍👦🇺🇸x", 3),
        "e\u{0301}👨‍👩‍👧‍👦🇺🇸"
    );

    // Deduplicated pending-subagent counter
    let items = vec![
        ("sub-1", Some("child-a")),
        ("sub-2", Some("child-a")),
        ("sub-3", Some("child-b")),
        ("watch-1", None),
        ("watch-2", None),
    ];
    let deduped = dedup_pending_subagent_threads(items, |(_, c)| c.map(str::to_owned));
    assert_eq!(deduped.len(), 4);
    assert_eq!(
        count_unique_pending_subagent_threads([
            Some("child-a"),
            Some("child-a"),
            Some("child-b"),
            None,
        ]),
        3
    );
}

#[test]
fn loopback_host_and_branded_desktop_control_cover_all_variants() {
    for valid in [
        "localhost",
        "LOCALHOST",
        "127.0.0.1",
        "127.0.0.2",
        "127.255.255.254",
        "::1",
        "[::1]",
        "[0:0:0:0:0:0:0:1]",
    ] {
        assert!(is_loopback_host(valid), "expected loopback for `{valid}`");
    }
    for invalid in [
        "",
        "0.0.0.0",
        "192.168.1.1",
        "127.evil.com",
        "127.0.0.999",
        "[::1",
        "::1]",
        "app.legion.codes",
    ] {
        assert!(
            !is_loopback_host(invalid),
            "expected non-loopback for `{invalid}`"
        );
    }

    let legion_win = resolve_desktop_app_control_address_for_brand(
        &LEGION_CODE,
        r"C:\Users\alice\.legioncode\userdata",
        DesktopControlPlatform::Win32,
        r"C:\Temp",
        Some(1000),
    );
    assert!(legion_win.address.starts_with(r"\\.\pipe\legioncode-app-"));

    let legion_linux = resolve_desktop_app_control_address_for_brand(
        &LEGION_CODE,
        "/home/alice/.legioncode/userdata",
        DesktopControlPlatform::Linux,
        "/run/user/1000",
        Some(1000),
    );
    assert!(legion_linux.address.contains("/legioncode-1000/"));
}
