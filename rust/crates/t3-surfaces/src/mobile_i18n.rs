//! Localized mobile workspace navigation and Live Activity presenters (`BUG-24`).
//!
//! Resolves locale selections through [`t3_edge::i18n`] across all 10 supported locales
//! (`en`, `de`, `es`, `fr`, `ja`, `ko`, `pt-BR`, `ru`, `sv`, `zh` / `zh-CN`) and eliminates
//! hardcoded English strings in `T3WorkspaceNavigationModule.kt`, `T3SidebarTabBarView.swift`,
//! `AgentActivityPresentation.kt`, and `AgentNotifications.kt`.

use serde::{Deserialize, Serialize};
use t3_edge::i18n::{
    DEFAULT_LOCALE, SYSTEM_LANGUAGE, lookup_exact_locale, lookup_locale, resolve_locale_selection,
};

use crate::mobile_i18n_nav_catalog::{NavCatalog, nav_catalog};

pub use crate::mobile_i18n_activity::{
    AgentActivityInput, AgentActivityPhase, AgentActivityRowInput, AgentLiveActivityPresentation,
    AgentLiveActivityPresenter, LocalizedAgentActivityRow,
};

/// Workspace navigation tab identifiers matching `T3WorkspaceNavigationModule.kt`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NavigationTabKind {
    /// Chat threads tab (`"threads"`).
    Threads,
    /// Pull requests tab (`"pullRequests"`).
    PullRequests,
    /// Issues tab (`"issues"`).
    Issues,
    /// Usage limits tab (`"usage"`).
    Usage,
    /// Settings tab (`"settings"`).
    Settings,
}

impl NavigationTabKind {
    /// Parses a tab key (`"threads"`, `"pullRequests"`, `"issues"`, `"usage"`, `"settings"`).
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "threads" => Some(Self::Threads),
            "pullRequests" => Some(Self::PullRequests),
            "issues" => Some(Self::Issues),
            "usage" => Some(Self::Usage),
            "settings" => Some(Self::Settings),
            _ => None,
        }
    }
}

/// Complete localized chrome strings for mobile workspace navigation (`BUG-24`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileNavigationStrings {
    /// Resolved canonical catalog locale code from `t3_edge::i18n`.
    pub locale: String,
    /// Compact label for `"threads"` (`"Chats"` in English iOS/Android bars).
    pub chats: String,
    /// Full label for `"threads"` (`"Threads"`).
    pub threads: String,
    /// Compact label for `"pullRequests"` (`"Pulls"`).
    pub pulls: String,
    /// Expanded rail label for `"pullRequests"` (`"Pull Requests"`).
    pub pull_requests: String,
    /// Short badge/iOS label for `"pullRequests"` (`"PRs"`).
    pub prs_short: String,
    /// Label for `"issues"` (`"Issues"`).
    pub issues: String,
    /// Label for `"usage"` (`"Usage"`).
    pub usage: String,
    /// Label for `"settings"` (`"Settings"`).
    pub settings: String,
    /// FAB/rail button label (`"New thread"`).
    pub new_thread: String,
    /// Compose button accessibility label (`"New chat"`).
    pub new_chat: String,
    /// Scrim accessibility description (`"Close navigation rail"`).
    pub close_rail: String,
    /// Rail toggle accessibility description when collapsed (`"Expand navigation rail"`).
    pub expand_rail: String,
    /// Rail toggle accessibility description when expanded (`"Collapse navigation rail"`).
    pub collapse_rail: String,
}

/// Resolves locale settings through [`t3_edge::i18n`] into localized navigation chrome (`BUG-24`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileNavigationPresenter {
    catalog_locale: &'static str,
    formatting_locale: String,
    strings: &'static NavCatalog,
}

impl MobileNavigationPresenter {
    /// Resolves a saved `language` setting and ordered `system_locales` through [`t3_edge::i18n`].
    ///
    /// Also matches regional tags such as `"zh-CN"` when passed directly as `language`, while
    /// preserving `t3_edge::i18n`'s Traditional Chinese exclusion (`zh-TW` does not map to `zh`).
    pub fn from_settings(language: &str, system_locales: &[&str]) -> Self {
        let (catalog_locale, formatting_locale) = resolve_surface_locale(language, system_locales);
        Self {
            catalog_locale,
            formatting_locale,
            strings: nav_catalog(catalog_locale),
        }
    }

    /// Resolves a single locale tag (e.g. `"es"`, `"pt-BR"`, `"zh-CN"`, `"sv"`).
    pub fn for_locale(tag: &str) -> Self {
        Self::from_settings(tag, &[])
    }

    /// Canonical `t3_edge::i18n` catalog locale code (`"en"`, `"sv"`, `"pt-BR"`, `"zh"`, ...).
    pub fn locale(&self) -> &'static str {
        self.catalog_locale
    }

    /// Regional formatting locale tag preserved from host preferences.
    pub fn formatting_locale(&self) -> &str {
        &self.formatting_locale
    }

    /// Compact tab title matching `T3WorkspaceNavigationView.tabTitle(tab)`.
    pub fn tab_title(&self, tab: NavigationTabKind) -> &'static str {
        match tab {
            NavigationTabKind::Threads => self.strings.chats,
            NavigationTabKind::PullRequests => self.strings.pulls,
            NavigationTabKind::Issues => self.strings.issues,
            NavigationTabKind::Usage => self.strings.usage,
            NavigationTabKind::Settings => self.strings.settings,
        }
    }

    /// Expanded rail destination title matching `T3WorkspaceNavigationView.expandedTabTitle(tab)`.
    pub fn expanded_tab_title(&self, tab: NavigationTabKind) -> &'static str {
        match tab {
            NavigationTabKind::PullRequests => self.strings.pull_requests,
            other => self.tab_title(other),
        }
    }

    /// Short iOS tab accessibility label (`T3SidebarTabBarView.swift`).
    pub fn ios_tab_label(&self, tab: NavigationTabKind) -> &'static str {
        match tab {
            NavigationTabKind::PullRequests => self.strings.prs_short,
            other => self.tab_title(other),
        }
    }

    /// Localized `"New thread"` label (`T3WorkspaceNavigationModule.kt:367`).
    pub fn new_thread_label(&self) -> &'static str {
        self.strings.new_thread
    }

    /// Localized `"New chat"` accessibility label (`T3SidebarTabBarView.swift:87`).
    pub fn new_chat_label(&self) -> &'static str {
        self.strings.new_chat
    }

    /// Localized scrim accessibility description (`"Close navigation rail"`).
    pub fn close_rail_label(&self) -> &'static str {
        self.strings.close_rail
    }

    /// Localized rail toggle accessibility description (`Expand` vs `Collapse`).
    pub fn rail_toggle_label(&self, expanded: bool) -> &'static str {
        if expanded {
            self.strings.collapse_rail
        } else {
            self.strings.expand_rail
        }
    }

    /// Materializes all localized navigation chrome strings as a serializable struct.
    pub fn strings(&self) -> MobileNavigationStrings {
        MobileNavigationStrings {
            locale: self.catalog_locale.to_owned(),
            chats: self.strings.chats.to_owned(),
            threads: self.strings.threads.to_owned(),
            pulls: self.strings.pulls.to_owned(),
            pull_requests: self.strings.pull_requests.to_owned(),
            prs_short: self.strings.prs_short.to_owned(),
            issues: self.strings.issues.to_owned(),
            usage: self.strings.usage.to_owned(),
            settings: self.strings.settings.to_owned(),
            new_thread: self.strings.new_thread.to_owned(),
            new_chat: self.strings.new_chat.to_owned(),
            close_rail: self.strings.close_rail.to_owned(),
            expand_rail: self.strings.expand_rail.to_owned(),
            collapse_rail: self.strings.collapse_rail.to_owned(),
        }
    }
}

pub(crate) fn resolve_surface_locale(
    language: &str,
    system_locales: &[&str],
) -> (&'static str, String) {
    let trimmed = language.trim();
    if !trimmed.is_empty() && !trimmed.eq_ignore_ascii_case(SYSTEM_LANGUAGE) {
        if let Some(exact) = lookup_exact_locale(trimmed) {
            return (exact.code, exact.code.to_owned());
        }
        if let Some(regional) = lookup_locale(trimmed) {
            return (regional.code, trimmed.replace('_', "-"));
        }
    }
    let selection = resolve_locale_selection(SYSTEM_LANGUAGE, system_locales);
    if selection.catalog_locale != DEFAULT_LOCALE || !system_locales.is_empty() {
        return (selection.catalog_locale, selection.formatting_locale);
    }
    (DEFAULT_LOCALE, DEFAULT_LOCALE.to_owned())
}
