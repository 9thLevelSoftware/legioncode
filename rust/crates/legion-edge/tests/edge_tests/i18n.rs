//! Tests for `legion_edge::i18n` (`BUG-27`).

use legion_edge::i18n::{
    DEFAULT_LOCALE, SUPPORTED_LOCALE_CODES, SUPPORTED_LOCALES, localized_href, lookup_exact_locale,
    lookup_locale, resolve_locale, resolve_locale_selection,
};
use proptest::prelude::*;

#[test]
fn bug27_supported_locales_match_all_10_locales_including_swedish() -> Result<(), String> {
    assert_eq!(SUPPORTED_LOCALES.len(), 10);
    assert_eq!(
        SUPPORTED_LOCALE_CODES,
        &[
            "en", "de", "es", "fr", "ja", "ko", "pt-BR", "ru", "sv", "zh"
        ]
    );

    let sv = lookup_exact_locale("sv").ok_or("Swedish locale missing")?;
    assert_eq!(sv.code, "sv");
    assert_eq!(sv.bcp47_tag, "sv");
    assert_eq!(sv.url_path, "sv");
    assert_eq!(sv.native_name, "Svenska");
    assert_eq!(localized_href(sv, "/"), "/sv");
    assert_eq!(localized_href(sv, "/download"), "/sv/download");

    let en = lookup_exact_locale("en").ok_or("English locale missing")?;
    assert_eq!(localized_href(en, "/"), "/");
    assert_eq!(localized_href(en, "/download"), "/download");

    let zh = lookup_exact_locale("zh-Hans").ok_or("zh-Hans missing")?;
    assert_eq!(zh.code, "zh");
    assert_eq!(zh.url_path, "zh");
    assert_eq!(localized_href(zh, "/"), "/zh");

    let pt_br = lookup_exact_locale("pt-BR").ok_or("pt-BR missing")?;
    assert_eq!(localized_href(pt_br, "/download"), "/pt-br/download");
    Ok(())
}

#[test]
fn locale_lookup_and_fallback_preserve_traditional_chinese_exclusion() {
    assert_eq!(resolve_locale("sv", &["en-US"]), "sv");
    assert_eq!(resolve_locale("system", &["sv-SE", "en-US"]), "sv");
    assert_eq!(resolve_locale("system", &["sv_SE"]), "sv");

    let pt = resolve_locale_selection("system", &["pt_BR"]);
    assert_eq!(pt.catalog_locale, "pt-BR");
    assert_eq!(pt.formatting_locale, "pt-BR");

    // `pt-PT` must not match `pt-BR`.
    assert_eq!(lookup_locale("pt-PT"), None);
    assert_eq!(resolve_locale("system", &["pt-PT", "de-DE"]), "de");

    // Traditional Chinese must never silently map to Simplified `zh`.
    for trad in ["zh-Hant", "zh-TW", "zh-HK", "zh-MO", "zh_Hant_TW"] {
        assert_eq!(lookup_locale(trad), None, "expected {trad} to return None");
    }
    assert_eq!(resolve_locale("system", &["zh-TW", "ja-JP"]), "ja");
    assert_eq!(resolve_locale("system", &["zh-CN"]), "zh");
    assert_eq!(resolve_locale("invalid", &[]), DEFAULT_LOCALE);
}

#[test]
fn supported_locales_have_unique_codes_paths_and_case_insensitive_system_setting() {
    use std::collections::BTreeSet;
    let mut codes = BTreeSet::new();
    let mut paths = BTreeSet::new();
    for entry in SUPPORTED_LOCALES {
        assert!(codes.insert(entry.code), "duplicate code {}", entry.code);
        assert!(
            paths.insert(entry.url_path),
            "duplicate path {}",
            entry.url_path
        );
        assert_ne!(entry.native_name, "");
        assert_ne!(entry.marketing_name, "");
    }
    assert_eq!(resolve_locale("SYSTEM", &["sv-SE"]), "sv");
    assert_eq!(resolve_locale("pt-br", &[]), "pt-BR");
}

proptest! {
    #[test]
    fn proptest_resolve_locale_always_returns_supported_code(
        language in "\\PC{0,24}",
        sys_a in "\\PC{0,24}",
        sys_b in "\\PC{0,24}",
    ) {
        let selected = resolve_locale(&language, &[sys_a.as_str(), sys_b.as_str()]);
        prop_assert!(SUPPORTED_LOCALE_CODES.contains(&selected));
    }
}
