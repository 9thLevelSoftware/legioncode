//! `Ctrl+K` command palette search (`t3_shared::search`) and item execution.

use t3_edge::i18n::SUPPORTED_LOCALES;
use t3_shared::search::{
    FieldMatchMode, FieldOptions, FuzzyPaletteIndex, PaletteEntry, PaletteField, SearchOptions,
};

use crate::actions::{RightPaneTab, WebAppAction};
use crate::state::WebAppState;
use crate::state_types::{CommandPaletteKind, CommandPaletteMatch, PaletteSearchRange};

struct PaletteCandidate {
    title: String,
    subtitle: String,
    keywords: String,
    kind: CommandPaletteKind,
    target: String,
    usage_boost: f64,
}

pub(crate) fn refresh_command_palette(state: &mut WebAppState) {
    let candidates = collect_palette_candidates(state);
    let query = state.command_palette.query.trim();
    if query.is_empty() {
        state.command_palette.results = candidates
            .into_iter()
            .take(16)
            .map(|c| CommandPaletteMatch {
                title: c.title,
                subtitle: c.subtitle,
                kind: c.kind,
                target: c.target,
                ranges: Vec::new(),
            })
            .collect();
        state.command_palette.selected_index = 0;
        return;
    }

    let entries: Vec<PaletteEntry<'_>> = candidates
        .iter()
        .map(|cand| PaletteEntry {
            fields: vec![
                PaletteField::primary(&cand.title),
                PaletteField {
                    text: &cand.subtitle,
                    options: FieldOptions {
                        weight: 0.7,
                        mode: FieldMatchMode::WordStart,
                        context: true,
                    },
                },
                PaletteField {
                    text: &cand.keywords,
                    options: FieldOptions {
                        weight: 0.6,
                        mode: FieldMatchMode::KeywordWordStart,
                        context: true,
                    },
                },
            ],
            usage_boost: cand.usage_boost,
        })
        .collect();
    let index = FuzzyPaletteIndex::from_entries(entries);
    let hits = index.search_with_options(
        query,
        SearchOptions {
            allow_typos: true,
            limit: Some(25),
        },
    );
    state.command_palette.results = hits
        .into_iter()
        .filter_map(|m| {
            let cand = candidates.get(m.item_index)?;
            Some(CommandPaletteMatch {
                title: cand.title.clone(),
                subtitle: cand.subtitle.clone(),
                kind: cand.kind,
                target: cand.target.clone(),
                ranges: m
                    .ranges
                    .into_iter()
                    .map(|r| PaletteSearchRange {
                        start: r.start,
                        end: r.end,
                    })
                    .collect(),
            })
        })
        .collect();
    state.command_palette.selected_index = 0;
}

fn collect_palette_candidates(state: &WebAppState) -> Vec<PaletteCandidate> {
    let mut items = Vec::with_capacity(10 + state.projects.len() + state.threads.len());
    push_static_candidates(state, &mut items);
    for project in &state.projects {
        items.push(PaletteCandidate {
            title: project.title.clone(),
            subtitle: format!("Project · {}", project.workspace_root),
            keywords: project.workspace_root.clone(),
            kind: CommandPaletteKind::SelectProject,
            target: project.id.clone(),
            usage_boost: 1.15,
        });
    }
    for thread in state.threads.values() {
        items.push(PaletteCandidate {
            title: thread.title.clone(),
            subtitle: format!("Thread · {}", thread.project_id),
            keywords: thread.branch.clone().unwrap_or_default(),
            kind: CommandPaletteKind::SelectThread,
            target: thread.id.clone(),
            usage_boost: 1.3,
        });
    }
    for desc in SUPPORTED_LOCALES {
        items.push(PaletteCandidate {
            title: format!("{} ({})", desc.native_name, desc.code),
            subtitle: state.locale.web.language_label.to_owned(),
            keywords: format!("locale language {} {}", desc.code, desc.bcp47_tag),
            kind: CommandPaletteKind::SetLocale,
            target: desc.code.to_owned(),
            usage_boost: 0.9,
        });
    }
    items
}

fn push_static_candidates(state: &WebAppState, items: &mut Vec<PaletteCandidate>) {
    let entries = [
        (
            state.locale.web.tab_chat,
            "Panel",
            "chat messages",
            CommandPaletteKind::SwitchTab,
            "chat",
            1.2,
        ),
        (
            state.locale.web.tab_diff,
            "Panel",
            "diff review patch",
            CommandPaletteKind::SwitchTab,
            "diff",
            1.2,
        ),
        (
            state.locale.web.tab_terminal,
            "Panel",
            "terminal vt pty",
            CommandPaletteKind::SwitchTab,
            "terminal",
            1.2,
        ),
        (
            state.locale.web.pairing_title,
            "Auth",
            "pair bearer token",
            CommandPaletteKind::FocusPairing,
            "",
            1.0,
        ),
        (
            state.locale.web.projects_title,
            "Sync",
            "fetch refresh projects",
            CommandPaletteKind::FetchProjects,
            "",
            1.0,
        ),
        (
            state.locale.web.terminal_export,
            "Terminal",
            "t3vt snapshot binary",
            CommandPaletteKind::ExportT3vt,
            "",
            1.0,
        ),
        (
            state.locale.web.terminal_reset,
            "Terminal",
            "reset clear vt",
            CommandPaletteKind::ResetVt,
            "",
            0.95,
        ),
        (
            state.locale.web.send_button,
            "Composer",
            "focus prompt message",
            CommandPaletteKind::FocusComposer,
            "",
            1.0,
        ),
        (
            &state.locale.nav.collapse_rail,
            "Layout",
            "sidebar rail toggle",
            CommandPaletteKind::ToggleSidebar,
            "",
            0.9,
        ),
    ];
    for (title, sub, kw, kind, target, boost) in entries {
        items.push(PaletteCandidate {
            title: title.to_owned(),
            subtitle: sub.to_owned(),
            keywords: kw.to_owned(),
            kind,
            target: target.to_owned(),
            usage_boost: boost,
        });
    }
}

pub(crate) fn command_palette_execute_action(
    state: &mut WebAppState,
    index: Option<usize>,
) -> Option<WebAppAction> {
    let idx = index.unwrap_or(state.command_palette.selected_index);
    let chosen = state.command_palette.results.get(idx).cloned()?;
    state.command_palette.open = false;
    Some(match chosen.kind {
        CommandPaletteKind::SwitchTab => WebAppAction::SetRightTab {
            tab: match chosen.target.as_str() {
                "diff" => RightPaneTab::Diff,
                "terminal" => RightPaneTab::Terminal,
                _ => RightPaneTab::Chat,
            },
        },
        CommandPaletteKind::SelectProject => WebAppAction::SelectProject {
            project_id: chosen.target,
        },
        CommandPaletteKind::SelectThread => WebAppAction::SelectThread {
            thread_id: chosen.target,
        },
        CommandPaletteKind::SetLocale => WebAppAction::SetLocale {
            language: chosen.target,
            system_locales: Vec::new(),
        },
        CommandPaletteKind::FetchProjects => WebAppAction::FetchProjects,
        CommandPaletteKind::ExportT3vt => WebAppAction::TerminalExportSnapshot,
        CommandPaletteKind::ResetVt => WebAppAction::TerminalReset,
        CommandPaletteKind::ToggleSidebar => WebAppAction::ToggleSidebar { open: None },
        CommandPaletteKind::FocusPairing | CommandPaletteKind::FocusComposer => return None,
    })
}
