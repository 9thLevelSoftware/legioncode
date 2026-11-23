//! Localized Live Activity & ongoing notification presenter (`BUG-24`).

use std::collections::BTreeSet;

use crate::mobile_i18n::resolve_surface_locale;
use crate::mobile_i18n_activity_catalog::{ActivityCatalog, activity_catalog};
use crate::mobile_i18n_activity_types::truncate_chars;

pub use crate::mobile_i18n_activity_types::{
    AgentActivityInput, AgentActivityPhase, AgentActivityRowInput, AgentLiveActivityPresentation,
    LocalizedAgentActivityRow,
};

/// Localized Live Activity / notification presenter backed by [`legion_edge::i18n`] (`BUG-24`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentLiveActivityPresenter {
    catalog_locale: &'static str,
    catalog: &'static ActivityCatalog,
}

impl AgentLiveActivityPresenter {
    /// Resolves `language` and `system_locales` through [`legion_edge::i18n`].
    pub fn from_settings(language: &str, system_locales: &[&str]) -> Self {
        let (catalog_locale, _) = resolve_surface_locale(language, system_locales);
        Self {
            catalog_locale,
            catalog: activity_catalog(catalog_locale),
        }
    }

    /// Resolves a single locale tag through [`legion_edge::i18n`].
    pub fn for_locale(tag: &str) -> Self {
        Self::from_settings(tag, &[])
    }

    /// Canonical `legion_edge::i18n` locale code.
    pub fn locale(&self) -> &'static str {
        self.catalog_locale
    }

    /// Localized short status label for `phase`.
    pub fn status_label(&self, phase: AgentActivityPhase) -> &'static str {
        match phase {
            AgentActivityPhase::Starting => self.catalog.phase_starting_status,
            AgentActivityPhase::Running => self.catalog.phase_running_status,
            AgentActivityPhase::WaitingForApproval => self.catalog.phase_approval_status,
            AgentActivityPhase::WaitingForInput => self.catalog.phase_input_status,
            AgentActivityPhase::Stale => self.catalog.phase_stale_status,
            AgentActivityPhase::Completed => self.catalog.phase_completed_status,
            AgentActivityPhase::Failed => self.catalog.phase_failed_status,
        }
    }

    /// Localized heading label for `phase`.
    pub fn heading_label(&self, phase: AgentActivityPhase) -> &'static str {
        match phase {
            AgentActivityPhase::Starting => self.catalog.phase_starting_heading,
            AgentActivityPhase::Running => self.catalog.phase_running_heading,
            AgentActivityPhase::WaitingForApproval => self.catalog.phase_approval_heading,
            AgentActivityPhase::WaitingForInput => self.catalog.phase_input_heading,
            AgentActivityPhase::Stale => self.catalog.phase_stale_heading,
            AgentActivityPhase::Completed => self.catalog.phase_completed_heading,
            AgentActivityPhase::Failed => self.catalog.phase_failed_heading,
        }
    }

    /// Localized action label for `phase`.
    pub fn action_label(&self, phase: AgentActivityPhase) -> &'static str {
        match phase {
            AgentActivityPhase::WaitingForApproval => self.catalog.phase_approval_action,
            AgentActivityPhase::WaitingForInput => self.catalog.phase_input_action,
            _ => self.catalog.action_open,
        }
    }

    /// Localized `"Dismiss"` label (`AgentNotifications.kt:314`).
    pub fn dismiss_label(&self) -> &'static str {
        self.catalog.action_dismiss
    }

    /// Localized `"Agent activity"` fallback title (`AgentActivityPresentation.kt:107`).
    pub fn fallback_title(&self) -> &'static str {
        self.catalog.summary_fallback
    }

    /// Renders a complete localized [`AgentLiveActivityPresentation`] for `input`.
    pub fn present(&self, input: &AgentActivityInput) -> AgentLiveActivityPresentation {
        let active_count = input
            .active_count
            .unwrap_or_else(|| input.rows.iter().filter(|r| !r.phase.is_finished()).count());
        let attention_count = input
            .attention_count
            .unwrap_or_else(|| input.rows.iter().filter(|r| r.phase.needs_user()).count());
        let failed_count = input
            .rows
            .iter()
            .filter(|r| r.phase == AgentActivityPhase::Failed)
            .count();
        let finished_count = input.rows.iter().filter(|r| r.phase.is_finished()).count();
        let thread_count = active_count + finished_count;
        let phase = input.phase.or_else(|| input.rows.first().map(|r| r.phase));

        let summary = self.format_summary(input, active_count, attention_count, failed_count);
        let chip = self.format_chip(input, phase, active_count);
        let action = input.active.then(|| {
            phase
                .map_or(self.catalog.action_open, |p| self.action_label(p))
                .to_owned()
        });
        let subtext = self.format_subtext(&input.rows, active_count, thread_count);
        let rows = input
            .rows
            .iter()
            .map(|r| LocalizedAgentActivityRow {
                phase: r.phase,
                status_label: self.status_label(r.phase).to_owned(),
                title: r.title.clone(),
                project: r.project.clone(),
            })
            .collect();

        AgentLiveActivityPresentation {
            locale: self.catalog_locale.to_owned(),
            summary,
            subtext,
            chip,
            action,
            dismiss_label: self.catalog.action_dismiss.to_owned(),
            phase_heading: phase.map(|p| self.heading_label(p).to_owned()),
            rows,
        }
    }

    fn format_summary(
        &self,
        input: &AgentActivityInput,
        active_count: usize,
        attention_count: usize,
        failed_count: usize,
    ) -> String {
        if let Some(hero) = input.rows.first() {
            if input.rows.len() == 1 {
                return hero.title.clone();
            }
            if attention_count > 0 {
                return format_count(
                    attention_count,
                    self.catalog.summary_need_you_one,
                    self.catalog.summary_need_you_other,
                );
            }
            if active_count > 0 && failed_count > 0 {
                return format_count(
                    failed_count,
                    self.catalog.summary_failed_one,
                    self.catalog.summary_failed_other,
                );
            }
            if active_count > 0 {
                return format_count(
                    active_count,
                    self.catalog.summary_working_one,
                    self.catalog.summary_working_other,
                );
            }
            if failed_count > 0 {
                let failed = format_count(
                    failed_count,
                    self.catalog.summary_failed_one,
                    self.catalog.summary_failed_other,
                );
                return format!("{}, {failed}", self.catalog.summary_finished_prefix);
            }
            return self.catalog.summary_all_finished.to_owned();
        }
        input
            .fallback_title
            .as_deref()
            .filter(|s| !s.is_empty())
            .map_or_else(
                || self.catalog.summary_fallback.to_owned(),
                |title| truncate_chars(title, 120),
            )
    }

    fn format_chip(
        &self,
        input: &AgentActivityInput,
        phase: Option<AgentActivityPhase>,
        active_count: usize,
    ) -> String {
        if !input.active {
            return if phase == Some(AgentActivityPhase::Failed) {
                self.catalog.phase_failed_status.to_owned()
            } else {
                self.catalog.phase_completed_status.to_owned()
            };
        }
        let Some(p) = phase else {
            return input
                .fallback_chip
                .as_deref()
                .filter(|s| !s.is_empty())
                .map_or_else(
                    || self.catalog.chip_active.to_owned(),
                    |chip| truncate_chars(chip, 7),
                );
        };
        if p == AgentActivityPhase::Running && active_count > 1 {
            let count_str = if active_count > 9 {
                "9+".to_owned()
            } else {
                active_count.to_string()
            };
            return self.catalog.chip_live_count.replace("{count}", &count_str);
        }
        match p {
            AgentActivityPhase::Starting => self.catalog.phase_running_status.to_owned(),
            AgentActivityPhase::WaitingForApproval => self.catalog.phase_approval_action.to_owned(),
            AgentActivityPhase::WaitingForInput => self.catalog.phase_input_action.to_owned(),
            other => self.status_label(other).to_owned(),
        }
    }

    fn format_subtext(
        &self,
        rows: &[AgentActivityRowInput],
        active_count: usize,
        thread_count: usize,
    ) -> Option<String> {
        if rows.len() <= 1 {
            return None;
        }
        let distinct_projects: BTreeSet<&str> = rows.iter().map(|r| r.project.as_str()).collect();
        let single_project = if distinct_projects.len() == 1 {
            rows.first()
                .map(|r| r.project.as_str())
                .filter(|s| !s.trim().is_empty())
        } else {
            None
        };
        let count_text = if active_count > 0 {
            format_count(
                active_count,
                self.catalog.subtext_active_one,
                self.catalog.subtext_active_other,
            )
        } else {
            format_count(
                thread_count,
                self.catalog.subtext_threads_one,
                self.catalog.subtext_threads_other,
            )
        };
        Some(match single_project {
            Some(project) => format!("{project} · {count_text}"),
            None => count_text,
        })
    }
}

fn format_count(count: usize, one: &str, other: &str) -> String {
    let template = if count == 1 { one } else { other };
    template.replace("{count}", &count.to_string())
}
