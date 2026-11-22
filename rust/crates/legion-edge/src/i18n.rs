//! Canonical locale registry and BCP-47 fallback parity across Web, Desktop, Mobile, and
//! Marketing (`BUG-27`).
//!
//! Mirrors `apps/{web,desktop,mobile}/gt.config.json`, `packages/client-runtime/src/locales.ts`,
//! and `apps/marketing/src/i18n/locales.ts` while fixing `BUG-27`: restoring Swedish (`sv`) so
//! all four surfaces support the exact same `en` + 9 non-English locales (`10` total).
//!
//! # Examples
//!
//! ```
//! use legion_edge::i18n::{
//!     DEFAULT_LOCALE, SUPPORTED_LOCALE_CODES, SUPPORTED_LOCALES, localized_href, lookup_locale,
//!     resolve_locale, resolve_locale_selection,
//! };
//!
//! assert_eq!(SUPPORTED_LOCALES.len(), 10);
//! assert_eq!(SUPPORTED_LOCALE_CODES.len(), 10);
//! assert!(SUPPORTED_LOCALE_CODES.contains(&"sv"));
//!
//! let sv = lookup_locale("sv-SE").ok_or("Swedish missing")?;
//! assert_eq!(sv.code, "sv");
//! assert_eq!(sv.native_name, "Svenska");
//! assert_eq!(localized_href(sv, "/download"), "/sv/download");
//!
//! // Traditional Chinese does not fall back to Simplified Chinese (`zh`).
//! assert_eq!(lookup_locale("zh-Hant-TW"), None);
//! assert_eq!(resolve_locale("system", &["zh-TW", "sv-SE"]), "sv");
//!
//! let sel = resolve_locale_selection("system", &["pt_BR"]);
//! assert_eq!(sel.catalog_locale, "pt-BR");
//! assert_eq!(sel.formatting_locale, "pt-BR");
//! assert_eq!(resolve_locale("unknown", &[]), DEFAULT_LOCALE);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use serde::{Deserialize, Serialize};

/// Canonical fallback language code (`"en"`).
pub const DEFAULT_LOCALE: &str = "en";

/// Sentinel setting value instructing the client to follow OS/browser language preferences.
pub const SYSTEM_LANGUAGE: &str = "system";

/// Metadata describing one supported `LegionCode` locale across app catalogs and marketing routes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LocaleDescriptor {
    /// Canonical catalog / `gt.config.json` code (`"en"`, `"pt-BR"`, `"sv"`, `"zh"`, ...).
    pub code: &'static str,
    /// BCP-47 tag emitted in HTML `lang` and `hreflang` (`"zh-Hans"` for `"zh"`).
    pub bcp47_tag: &'static str,
    /// Marketing URL path segment (`""` for English root, `"pt-br"` for `"pt-BR"`, `"sv"`, ...).
    pub url_path: &'static str,
    /// Native endonym shown in language selectors (`"Svenska"`, `"Deutsch"`, ...).
    pub native_name: &'static str,
    /// Localized marketing display name (`"简体中文"` vs `"中文（简体）"`).
    pub marketing_name: &'static str,
}

/// Canonical 10-locale list (`en` + all 9 non-English locales including Swedish `sv`, `BUG-27`).
pub const SUPPORTED_LOCALES: &[LocaleDescriptor] = &[
    LocaleDescriptor {
        code: "en",
        bcp47_tag: "en",
        url_path: "",
        native_name: "English",
        marketing_name: "English",
    },
    LocaleDescriptor {
        code: "de",
        bcp47_tag: "de",
        url_path: "de",
        native_name: "Deutsch",
        marketing_name: "Deutsch",
    },
    LocaleDescriptor {
        code: "es",
        bcp47_tag: "es",
        url_path: "es",
        native_name: "Español",
        marketing_name: "Español",
    },
    LocaleDescriptor {
        code: "fr",
        bcp47_tag: "fr",
        url_path: "fr",
        native_name: "Français",
        marketing_name: "Français",
    },
    LocaleDescriptor {
        code: "ja",
        bcp47_tag: "ja",
        url_path: "ja",
        native_name: "日本語",
        marketing_name: "日本語",
    },
    LocaleDescriptor {
        code: "ko",
        bcp47_tag: "ko",
        url_path: "ko",
        native_name: "한국어",
        marketing_name: "한국어",
    },
    LocaleDescriptor {
        code: "pt-BR",
        bcp47_tag: "pt-BR",
        url_path: "pt-br",
        native_name: "Português (Brasil)",
        marketing_name: "Português (Brasil)",
    },
    LocaleDescriptor {
        code: "ru",
        bcp47_tag: "ru",
        url_path: "ru",
        native_name: "Русский",
        marketing_name: "Русский",
    },
    LocaleDescriptor {
        code: "sv",
        bcp47_tag: "sv",
        url_path: "sv",
        native_name: "Svenska",
        marketing_name: "Svenska",
    },
    LocaleDescriptor {
        code: "zh",
        bcp47_tag: "zh-Hans",
        url_path: "zh",
        native_name: "中文（简体）",
        marketing_name: "简体中文",
    },
];

/// Canonical `gt.config.json` / client-runtime locale codes (`"en"` + 9 non-English locales).
pub const SUPPORTED_LOCALE_CODES: &[&str] = &[
    "en", "de", "es", "fr", "ja", "ko", "pt-BR", "ru", "sv", "zh",
];

/// Resolved catalog and regional formatting locales (`packages/client-runtime/src/locales.ts:43-55`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocaleSelection {
    /// Supported catalog code (`"en"`, `"sv"`, `"pt-BR"`, `"zh"`, ...).
    pub catalog_locale: &'static str,
    /// Original normalized BCP-47 tag preserved for regional number/date formatting.
    pub formatting_locale: String,
}

fn is_traditional_chinese(normalized_hyphen: &str) -> bool {
    let mut parts = normalized_hyphen.split('-');
    let Some(lang) = parts.next() else {
        return false;
    };
    let Some(second) = parts.next() else {
        return false;
    };
    lang == "zh" && matches!(second, "hant" | "tw" | "hk" | "mo")
}

/// Looks up a locale strictly by exact canonical `code` or `bcp47_tag` (case-insensitive).
pub fn lookup_exact_locale(tag: &str) -> Option<&'static LocaleDescriptor> {
    let normalized = tag.trim().replace('_', "-");
    if normalized.is_empty() {
        return None;
    }
    SUPPORTED_LOCALES.iter().find(|entry| {
        entry.code.eq_ignore_ascii_case(&normalized)
            || entry.bcp47_tag.eq_ignore_ascii_case(&normalized)
    })
}

/// Matches a BCP-47 or POSIX locale tag against [`SUPPORTED_LOCALES`], respecting regional
/// fallbacks while refusing to match Traditional Chinese (`zh-Hant`/`zh-TW`/`zh-HK`/`zh-MO`) to
/// Simplified `zh`.
pub fn lookup_locale(tag: &str) -> Option<&'static LocaleDescriptor> {
    let normalized = tag.trim().to_ascii_lowercase().replace('_', "-");
    if normalized.is_empty() {
        return None;
    }
    if let Some(exact) = SUPPORTED_LOCALES.iter().find(|entry| {
        entry.code.eq_ignore_ascii_case(&normalized)
            || entry.bcp47_tag.eq_ignore_ascii_case(&normalized)
    }) {
        return Some(exact);
    }
    if is_traditional_chinese(&normalized) {
        return None;
    }
    let primary = normalized.split('-').next()?;
    SUPPORTED_LOCALES
        .iter()
        .find(|entry| entry.code.eq_ignore_ascii_case(primary))
}

/// Resolves a saved `language` setting and ordered host `system_locales` into a supported
/// `catalog_locale` plus regional `formatting_locale` (`locales.ts:43-55`).
pub fn resolve_locale_selection(language: &str, system_locales: &[&str]) -> LocaleSelection {
    let trimmed = language.trim();
    if !trimmed.eq_ignore_ascii_case(SYSTEM_LANGUAGE)
        && let Some(exact) = lookup_exact_locale(trimmed)
    {
        return LocaleSelection {
            catalog_locale: exact.code,
            formatting_locale: exact.code.to_owned(),
        };
    }

    for tag in system_locales {
        if let Some(matched) = lookup_locale(tag) {
            let formatting_locale = tag.trim().replace('_', "-");
            return LocaleSelection {
                catalog_locale: matched.code,
                formatting_locale,
            };
        }
    }

    LocaleSelection {
        catalog_locale: DEFAULT_LOCALE,
        formatting_locale: DEFAULT_LOCALE.to_owned(),
    }
}

/// Resolves a saved `language` setting and ordered host `system_locales` to a supported catalog code.
pub fn resolve_locale(language: &str, system_locales: &[&str]) -> &'static str {
    resolve_locale_selection(language, system_locales).catalog_locale
}

/// Builds a marketing site localized path matching `apps/marketing/src/i18n/locales.ts:30-34`.
pub fn localized_href(locale: &LocaleDescriptor, path: &str) -> String {
    if locale.url_path.is_empty() {
        return path.to_owned();
    }
    if path == "/" {
        return format!("/{}", locale.url_path);
    }
    format!("/{}{path}", locale.url_path)
}
