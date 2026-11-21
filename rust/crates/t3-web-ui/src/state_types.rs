//! Workspace domain sub-states for [`crate::state::WebAppState`].

use serde::{Deserialize, Serialize};
use t3_client_core::diff_markdown::DiffLineKind;
use t3_edge::i18n::{DEFAULT_LOCALE, lookup_locale, resolve_locale_selection};
use t3_surfaces::mobile_composer::ComposerClipboardContext;
use t3_surfaces::mobile_diff::AttributedDiffRun;
use t3_surfaces::mobile_i18n::{
    AgentActivityInput, AgentLiveActivityPresentation, AgentLiveActivityPresenter,
    MobileNavigationPresenter, MobileNavigationStrings,
};

use crate::i18n_web::{WebCopyCatalog, web_copy_for_locale};

/// Resolved locale metadata and localized catalogs across all 10 `t3_edge::i18n` locales.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocaleState {
    /// Raw language preference (`"system"`, `"sv"`, `"pt-BR"`, ...).
    pub requested_language: String,
    /// Canonical `t3_edge::i18n` catalog code (`"en"`, `"de"`, `"es"`, `"fr"`, `"ja"`, `"ko"`, `"pt-BR"`, `"ru"`, `"sv"`, `"zh"`).
    pub catalog_locale: String,
    /// BCP-47 tag for `<html lang="...">` (`"zh-Hans"` for `"zh"`).
    pub bcp47_tag: String,
    /// Regional formatting locale preserved from host settings.
    pub formatting_locale: String,
    /// Endonym displayed in the language selector.
    pub native_name: String,
    /// Localized navigation chrome strings from `t3_surfaces::mobile_i18n`.
    pub nav: MobileNavigationStrings,
    /// Localized web chrome copy catalog.
    pub web: WebCopyCatalog,
}

impl LocaleState {
    /// Resolves `language` and ordered `system_locales` across `t3_edge::i18n` and `t3_surfaces`.
    pub fn resolve(language: &str, system_locales: &[&str]) -> Self {
        let nav_presenter = MobileNavigationPresenter::from_settings(language, system_locales);
        let catalog_locale = nav_presenter.locale();
        let formatting_locale = nav_presenter.formatting_locale().to_owned();
        let sel = resolve_locale_selection(language, system_locales);
        let descriptor = lookup_locale(catalog_locale)
            .or_else(|| lookup_locale(sel.catalog_locale))
            .or_else(|| lookup_locale(DEFAULT_LOCALE));
        let (bcp47_tag, native_name) = match descriptor {
            Some(desc) => (desc.bcp47_tag.to_owned(), desc.native_name.to_owned()),
            None => (DEFAULT_LOCALE.to_owned(), "English".to_owned()),
        };
        Self {
            requested_language: language.trim().to_owned(),
            catalog_locale: catalog_locale.to_owned(),
            bcp47_tag,
            formatting_locale,
            native_name,
            nav: nav_presenter.strings(),
            web: web_copy_for_locale(catalog_locale).clone(),
        }
    }

    /// Formats localized Live Activity summary for the sidebar/header status pill (`BUG-24`).
    pub fn present_activity(&self, input: &AgentActivityInput) -> AgentLiveActivityPresentation {
        AgentLiveActivityPresenter::for_locale(&self.catalog_locale).present(input)
    }
}

/// Authentication & pairing session state.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthState {
    /// Active bearer session token (`Authorization: Bearer ...`), if paired.
    #[serde(skip_serializing)]
    pub bearer_token: Option<String>,
    /// Current pairing token text input.
    pub pairing_input: String,
    /// Whether the session holds an active bearer token.
    pub paired: bool,
    /// Exchanged session ID reported by `/api/auth/pair`.
    pub session_id: Option<String>,
    /// Authenticated principal role (`"admin"`, `"client"`, ...).
    pub role: Option<String>,
    /// Granted authorization scopes.
    pub scopes: Vec<String>,
    /// Last authentication or pairing error banner.
    pub last_error: Option<String>,
}

/// Serializable auth summary omitting raw secret bearer tokens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthSnapshot {
    /// Whether the session holds an active bearer token.
    pub paired: bool,
    /// Whether a bearer token is currently stored in memory.
    pub has_bearer_token: bool,
    /// Exchanged session ID reported by `/api/auth/pair`.
    pub session_id: Option<String>,
    /// Authenticated principal role.
    pub role: Option<String>,
    /// Granted authorization scopes.
    pub scopes: Vec<String>,
    /// Current pairing input value.
    pub pairing_input: String,
    /// Last authentication error banner.
    pub last_error: Option<String>,
}

/// Workspace project summary synced from `/api/projects`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSummary {
    /// Stable project ID.
    pub id: String,
    /// Display title.
    pub title: String,
    /// Absolute workspace root directory path.
    pub workspace_root: String,
    /// Optional pin timestamp.
    pub pinned_at: Option<String>,
    /// Optional archive timestamp.
    pub archived_at: Option<String>,
    /// Last update timestamp.
    pub updated_at: String,
}

/// Thread summary row stored in sidebar and steady-shell caches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSummary {
    /// Thread identifier.
    pub id: String,
    /// Owning project identifier.
    pub project_id: String,
    /// Display title.
    pub title: String,
    /// Attached git branch, if any.
    pub branch: Option<String>,
    /// Active model name, if any.
    pub model: Option<String>,
    /// Whether the thread timeline has received `StreamItem::Synchronized`.
    pub synchronized: bool,
    /// Latest orchestration sequence observed for this thread.
    pub last_sequence: u64,
}

/// Single rendered timeline item inside a thread chat stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineMessage {
    /// Unique message or turn-item identifier.
    pub id: String,
    /// Role badge (`"user"`, `"assistant"`, `"system"`, `"tool"`).
    pub role: String,
    /// Item kind (`"user_message"`, `"assistant_message"`, `"command_execution"`, `"unknown"`, ...).
    pub kind: String,
    /// Optional card header or command summary.
    pub title: Option<String>,
    /// Body text or command output.
    pub body: String,
    /// Whether the item is still streaming.
    pub streaming: bool,
    /// Orchestration event sequence or ordinal.
    pub sequence: u64,
    /// Lifecycle status label (`"running"`, `"completed"`, `"failed"`, ...).
    pub status: Option<String>,
}

/// Materialized projection timeline for one thread.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadTimeline {
    /// Owning thread identifier.
    pub thread_id: String,
    /// Highest committed sequence observed for this thread.
    pub last_sequence: u64,
    /// Whether live stream has caught up (`StreamItem::Synchronized`).
    pub synchronized: bool,
    /// Count of forward-compatible `StreamItem::UnknownEvent` items skipped (`BUG-18`).
    pub unknown_event_count: u64,
    /// Active run status string (`"running"`, `"completed"`, ...).
    pub active_run_status: Option<String>,
    /// Ordered timeline entries rendered in the chat view.
    pub messages: Vec<TimelineMessage>,
}

/// Composer context pill normalized through `t3_surfaces::mobile_composer` (`BUG-23`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposerContextPill {
    /// Normalized `(contextId, screenshotContextId)` pair.
    pub context: ComposerClipboardContext,
    /// Display label.
    pub label: String,
    /// Optional workspace file path.
    pub path: Option<String>,
}

/// Composer draft and sanitized paste state.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposerState {
    /// Current textarea draft.
    pub draft: String,
    /// Active context pills (`BUG-23` normalized).
    pub context_pills: Vec<ComposerContextPill>,
    /// Most recent clipboard paste after `t3_terminal_vt::sanitize_bracketed_paste`.
    pub last_sanitized_paste: Option<String>,
    /// Most recent filtered `<pre data-t3-context-fragment="...">` clipboard HTML (`BUG-23`).
    pub last_clipboard_html: Option<String>,
}

/// Single decorated diff line carrying UTF-16 word-diff runs (`BUG-22`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DecoratedDiffLine {
    /// Context, addition, deletion, or hunk header.
    pub kind: DiffLineKind,
    /// 1-based old file line number, if applicable.
    pub old_line_number: Option<u32>,
    /// 1-based new file line number, if applicable.
    pub new_line_number: Option<u32>,
    /// Decorated UTF-16 runs preserving `FontStyleFlags` and word-highlight flags.
    pub runs: Vec<AttributedDiffRun>,
}

/// Decorated diff file ready for HTML rendering and JSON inspection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DecoratedDiffFile {
    /// File path (preserving leading/trailing spaces).
    pub name: String,
    /// Previous path when renamed.
    pub prev_name: Option<String>,
    /// Total added lines.
    pub additions: usize,
    /// Total deleted lines.
    pub deletions: usize,
    /// Decorated lines with UTF-16 word highlights (`BUG-22`).
    pub lines: Vec<DecoratedDiffLine>,
}

/// Diff review tab state.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffState {
    /// Last raw unified patch loaded.
    pub raw_patch: String,
    /// Parsed and decorated diff files.
    pub files: Vec<DecoratedDiffFile>,
    /// Currently selected diff file tab index.
    pub selected_file_index: usize,
}

/// Action kind executed when selecting a command palette result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CommandPaletteKind {
    /// Switch active right-pane tab (`chat` / `diff` / `terminal`).
    SwitchTab,
    /// Select a project by ID.
    SelectProject,
    /// Select a thread by ID.
    SelectThread,
    /// Switch UI locale by canonical code.
    SetLocale,
    /// Focus the pairing input form.
    FocusPairing,
    /// Refresh `/api/projects`.
    FetchProjects,
    /// Export a `"T3VT"` v2 binary snapshot.
    ExportT3vt,
    /// Reset the VT terminal grid.
    ResetVt,
    /// Focus the chat composer input.
    FocusComposer,
    /// Toggle the sidebar drawer.
    ToggleSidebar,
}

/// UTF-16 highlight span returned by `t3_shared::search::FuzzyPaletteIndex`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaletteSearchRange {
    /// Inclusive UTF-16 start offset in `title`.
    pub start: usize,
    /// Exclusive UTF-16 end offset in `title`.
    pub end: usize,
}

/// Ranked command palette entry displayed in the `Ctrl+K` modal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandPaletteMatch {
    /// Primary display title.
    pub title: String,
    /// Secondary category/context subtitle.
    pub subtitle: String,
    /// Action discriminant.
    pub kind: CommandPaletteKind,
    /// Optional target identifier (`tab`, `project_id`, `thread_id`, `locale`).
    pub target: String,
    /// UTF-16 highlight spans inside `title`.
    pub ranges: Vec<PaletteSearchRange>,
}

/// State of the `Ctrl+K` command palette modal.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandPaletteState {
    /// Whether the modal is currently open.
    pub open: bool,
    /// Current search query string.
    pub query: String,
    /// Currently highlighted result row index.
    pub selected_index: usize,
    /// Ranked fuzzy search matches.
    pub results: Vec<CommandPaletteMatch>,
}
