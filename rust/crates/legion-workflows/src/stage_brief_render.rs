#![forbid(unsafe_code)]
//! Markdown rendering helpers for Spec and Plan stage briefs.

use std::fmt::Write as _;

use legion_contracts::workflow_board::{
    WorkflowBoardAcceptanceCriterion, WorkflowBoardAssistanceMode, WorkflowBoardPlanAlternative,
    WorkflowBoardPlanLensEntry, WorkflowBoardPlanningStrategy, WorkflowBoardTaskGranularity,
};

use crate::stage_brief::{BriefChallengeConcern, PlanBriefInput, SpecBriefInput};

pub(crate) fn assist_label(mode: WorkflowBoardAssistanceMode) -> &'static str {
    match mode {
        WorkflowBoardAssistanceMode::Guided => "guided",
        WorkflowBoardAssistanceMode::Balanced => "balanced",
        WorkflowBoardAssistanceMode::Direct => "direct",
    }
}

pub(crate) fn strategy_label(strategy: WorkflowBoardPlanningStrategy) -> &'static str {
    match strategy {
        WorkflowBoardPlanningStrategy::Outcomes => "outcomes",
        WorkflowBoardPlanningStrategy::RiskFirst => "risk-first",
        WorkflowBoardPlanningStrategy::ExpandContract => "expand-contract",
        WorkflowBoardPlanningStrategy::Custom => "custom",
    }
}

pub(crate) fn granularity_label(granularity: WorkflowBoardTaskGranularity) -> &'static str {
    match granularity {
        WorkflowBoardTaskGranularity::Coarse => "coarse",
        WorkflowBoardTaskGranularity::Balanced => "balanced",
        WorkflowBoardTaskGranularity::Fine => "fine",
    }
}

pub(crate) fn append_list_section(
    md: &mut String,
    heading: &str,
    items: &[String],
    empty_line: &str,
) {
    let _ = writeln!(md, "## {heading}");
    if items.is_empty() {
        let _ = writeln!(md, "{empty_line}\n");
    } else {
        for item in items {
            let _ = writeln!(md, "- {}", item.trim());
        }
        let _ = writeln!(md);
    }
}

pub(crate) fn append_code_list_section(
    md: &mut String,
    heading: &str,
    items: &[String],
    empty_line: &str,
) {
    let _ = writeln!(md, "## {heading}");
    if items.is_empty() {
        let _ = writeln!(md, "{empty_line}\n");
    } else {
        for item in items {
            let _ = writeln!(md, "- `{}`", item.trim());
        }
        let _ = writeln!(md);
    }
}

pub(crate) fn append_ac_section(md: &mut String, acs: &[WorkflowBoardAcceptanceCriterion]) {
    let _ = writeln!(md, "## Acceptance Criteria");
    if acs.is_empty() {
        let _ = writeln!(md, "- None specified.\n");
    } else {
        for ac in acs {
            let _ = writeln!(md, "- **{}**: {}", ac.id.trim(), ac.text.trim());
        }
        let _ = writeln!(md);
    }
}

pub(crate) fn render_spec_brief_markdown(input: &SpecBriefInput) -> String {
    let mut md = String::new();
    let _ = writeln!(md, "# Spec Brief: {}\n", input.card_title.trim());
    let _ = writeln!(md, "## Metadata");
    let _ = writeln!(md, "- Card ID: `{}`", input.card_id.trim());
    let _ = writeln!(md, "- Project ID: `{}`", input.project_id.trim());
    let _ = writeln!(md, "- Card Revision: `{}`", input.card_revision);
    let _ = writeln!(
        md,
        "- Assistance Mode: `{}`",
        assist_label(input.assistance_mode)
    );
    if let Some(goal) = input.intake_goal {
        let _ = writeln!(md, "- Intake Goal: `{goal:?}`");
    }
    let _ = writeln!(md, "- Workspace Root: `{}`", input.workspace_root.trim());
    let _ = writeln!(
        md,
        "- Worktree Path: `{}`",
        opt_non_empty(input.worktree_path.as_deref(), "(none)")
    );
    let _ = writeln!(
        md,
        "- Branch: `{}`\n",
        opt_non_empty(input.branch.as_deref(), "(default)")
    );

    let _ = writeln!(md, "## Current Problem / Description");
    if input.card_description.trim().is_empty() {
        let _ = writeln!(md, "_No initial brief provided._\n");
    } else {
        let _ = writeln!(md, "{}\n", input.card_description.trim());
    }

    if let Some(existing) = input
        .current_spec_markdown
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let _ = writeln!(md, "## Prior / Current Spec Draft (`spec.md`)");
        let _ = writeln!(md, "```markdown\n{existing}\n```\n");
    }

    append_ac_section(&mut md, &input.acceptance_criteria);
    append_list_section(
        &mut md,
        "Constraints",
        &input.constraints,
        "- None specified.",
    );
    append_list_section(
        &mut md,
        "Unresolved Decisions",
        &input.unresolved_decisions,
        "- None.",
    );
    append_code_list_section(&mut md, "Source References", &input.source_refs, "- None.");

    let _ = writeln!(md, "## Wireframes");
    match input
        .wireframes_index
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(idx) => {
            let _ = writeln!(md, "- Index: `{idx}`\n");
        }
        None => {
            let _ = writeln!(md, "- None (non-UI or wireframes omitted).\n");
        }
    }

    let _ = writeln!(md, "## Expected Spec Output Contract");
    if input.expected_output_contract.trim().is_empty() {
        let _ = writeln!(
            md,
            "- Use `legion_workflow_board_read_brief` and `legion_workflow_board_propose` (`stage: \"spec\"`)."
        );
        let _ = writeln!(
            md,
            "- Never self-approve the spec or run `git commit` on `.legion/workflows/`."
        );
    } else {
        let _ = writeln!(md, "{}", input.expected_output_contract.trim());
    }
    md
}

pub(crate) fn append_plan_m2_context(md: &mut String, input: &PlanBriefInput) {
    if let Some(exp) = input
        .exploration_summary
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let _ = writeln!(md, "## Guided Exploration Summary\n{exp}\n");
    }
    if !input.resolved_decisions.is_empty() {
        append_list_section(
            md,
            "Resolved Decisions",
            &input.resolved_decisions,
            "- None.",
        );
    }
    if let Some(ch_sum) = input
        .challenge_receipt_summary
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let _ = writeln!(md, "## Challenge Summary\n{ch_sum}\n");
    }
    append_challenge_concerns(md, &input.challenge_concerns);
    append_lenses_section(md, &input.design_lenses);
    append_alternatives_section(
        md,
        &input.design_alternatives,
        input.selected_alternative_id.as_deref(),
        input.selection_rationale.as_deref(),
    );
    if !input.existing_tasks_summary.is_empty() {
        append_list_section(
            md,
            "Existing Tasks Summary",
            &input.existing_tasks_summary,
            "- None.",
        );
    }
}

fn append_challenge_concerns(md: &mut String, concerns: &[BriefChallengeConcern]) {
    if concerns.is_empty() {
        return;
    }
    let _ = writeln!(md, "## Challenge Concerns & Operator Dispositions");
    for c in concerns {
        let note = c
            .operator_note
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| format!(" — `{s}`"))
            .unwrap_or_default();
        let _ = writeln!(
            md,
            "- **{}** [{}]: {}{}",
            c.id.trim(),
            c.disposition.trim(),
            c.summary.trim(),
            note
        );
    }
    let _ = writeln!(md);
}

fn append_lenses_section(md: &mut String, lenses: &[WorkflowBoardPlanLensEntry]) {
    if lenses.is_empty() {
        return;
    }
    let _ = writeln!(md, "## Design Lenses");
    for l in lenses {
        let state = match l.applicable {
            Some(true) => format!("applicable: {}", l.summary.as_deref().unwrap_or("").trim()),
            Some(false) => format!(
                "not-applicable: {}",
                l.reason.as_deref().unwrap_or("").trim()
            ),
            None => "undecided".to_owned(),
        };
        let _ = writeln!(md, "- {:?}: {}", l.lens, state);
    }
    let _ = writeln!(md);
}

fn append_alternatives_section(
    md: &mut String,
    alts: &[WorkflowBoardPlanAlternative],
    selected_id: Option<&str>,
    rationale: Option<&str>,
) {
    if alts.is_empty() && selected_id.is_none() {
        return;
    }
    let _ = writeln!(md, "## Design Alternatives");
    for alt in alts {
        let _ = writeln!(
            md,
            "- **{}** (`{}`): {} (Tradeoffs: {})",
            alt.title.trim(),
            alt.id.trim(),
            alt.summary.trim(),
            alt.tradeoffs.trim()
        );
    }
    if let Some(sel) = selected_id.map(str::trim).filter(|s| !s.is_empty()) {
        let _ = writeln!(md, "- Selected Alternative: `{sel}`");
    }
    if let Some(rat) = rationale.map(str::trim).filter(|s| !s.is_empty()) {
        let _ = writeln!(md, "- Selection Rationale: {rat}");
    }
    let _ = writeln!(md);
}

pub(crate) fn opt_non_empty<'a>(val: Option<&'a str>, fallback: &'a str) -> &'a str {
    val.map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(fallback)
}
