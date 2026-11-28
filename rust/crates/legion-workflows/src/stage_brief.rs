#![forbid(unsafe_code)]
//! Deterministic Spec and Plan stage brief assembly and SHA-256 digest calculation.

use std::fmt::Write as _;

use legion_contracts::workflow_board::{
    WorkflowBoardAcceptanceCriterion, WorkflowBoardAssistanceMode, WorkflowBoardIntakeGoal,
    WorkflowBoardPlanAlternative, WorkflowBoardPlanLensEntry, WorkflowBoardPlanningStrategy,
    WorkflowBoardTaskGranularity,
};
use sha2::{Digest, Sha256};

use crate::stage_brief_render::{
    append_ac_section, append_code_list_section, append_list_section, append_plan_m2_context,
    assist_label, granularity_label, opt_non_empty, render_spec_brief_markdown, strategy_label,
};

/// Challenge concern summary embedded in [`PlanBriefInput`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BriefChallengeConcern {
    pub id: String,
    pub summary: String,
    pub disposition: String,
    pub operator_note: Option<String>,
}

/// Pure inputs required to assemble a deterministic Spec stage brief.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecBriefInput {
    pub card_id: String,
    pub card_title: String,
    pub card_description: String,
    pub project_id: String,
    pub workspace_root: String,
    pub worktree_path: Option<String>,
    pub branch: Option<String>,
    pub card_revision: u64,
    pub assistance_mode: WorkflowBoardAssistanceMode,
    pub intake_goal: Option<WorkflowBoardIntakeGoal>,
    pub current_spec_markdown: Option<String>,
    pub acceptance_criteria: Vec<WorkflowBoardAcceptanceCriterion>,
    pub constraints: Vec<String>,
    pub source_refs: Vec<String>,
    pub wireframes_index: Option<String>,
    pub unresolved_decisions: Vec<String>,
    pub expected_output_contract: String,
}

/// Rendered Spec brief markdown and its `"sha256:{hex}"` content digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssembledSpecBrief {
    pub markdown: String,
    pub brief_digest: String,
}

/// Pure inputs required to assemble a deterministic Plan stage brief.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanBriefInput {
    pub card_id: String,
    pub card_title: String,
    pub project_id: String,
    pub workspace_root: String,
    pub worktree_path: Option<String>,
    pub branch: Option<String>,
    pub spec_revision: u64,
    pub spec_digest: String,
    pub spec_brief: String,
    pub spec_markdown: String,
    pub acceptance_criteria: Vec<WorkflowBoardAcceptanceCriterion>,
    pub constraints: Vec<String>,
    pub unresolved_decisions: Vec<String>,
    pub source_refs: Vec<String>,
    pub wireframes_index: Option<String>,
    pub assistance_mode: WorkflowBoardAssistanceMode,
    pub planning_strategy: Option<WorkflowBoardPlanningStrategy>,
    pub strategy_rationale: Option<String>,
    pub task_granularity: Option<WorkflowBoardTaskGranularity>,
    pub exploration_summary: Option<String>,
    pub resolved_decisions: Vec<String>,
    pub challenge_receipt_summary: Option<String>,
    pub challenge_concerns: Vec<BriefChallengeConcern>,
    pub design_lenses: Vec<WorkflowBoardPlanLensEntry>,
    pub design_alternatives: Vec<WorkflowBoardPlanAlternative>,
    pub selected_alternative_id: Option<String>,
    pub selection_rationale: Option<String>,
    pub existing_tasks_summary: Vec<String>,
    pub expected_output_contract: String,
}

/// Rendered Plan brief markdown and its `"sha256:{hex}"` content digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssembledPlanBrief {
    pub markdown: String,
    pub brief_digest: String,
}

/// Computes `"sha256:{hex}"` over `bytes`.
#[must_use]
pub fn sha256_prefixed(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity("sha256:".len() + digest.len() * 2);
    out.push_str("sha256:");
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Deterministically builds the Spec stage brief markdown and SHA-256 digest.
#[must_use]
pub fn build_spec_brief(input: &SpecBriefInput) -> AssembledSpecBrief {
    let markdown = render_spec_brief_markdown(input);
    let brief_digest = sha256_prefixed(markdown.as_bytes());
    AssembledSpecBrief {
        markdown,
        brief_digest,
    }
}

/// Deterministically builds the Plan stage brief markdown and SHA-256 digest.
#[must_use]
pub fn build_plan_brief(input: &PlanBriefInput) -> AssembledPlanBrief {
    let mut md = String::new();
    let _ = writeln!(md, "# Plan Brief: {}\n", input.card_title.trim());
    let _ = writeln!(md, "## Metadata");
    let _ = writeln!(md, "- Card ID: `{}`", input.card_id.trim());
    let _ = writeln!(md, "- Project ID: `{}`", input.project_id.trim());
    let _ = writeln!(md, "- Approved Spec Revision: `{}`", input.spec_revision);
    let _ = writeln!(md, "- Approved Spec Digest: `{}`", input.spec_digest.trim());
    let _ = writeln!(
        md,
        "- Assistance Mode: `{}`",
        assist_label(input.assistance_mode)
    );
    if let Some(strategy) = input.planning_strategy {
        let _ = writeln!(md, "- Planning Strategy: `{}`", strategy_label(strategy));
    }
    if let Some(rat) = input
        .strategy_rationale
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let _ = writeln!(md, "- Strategy Rationale: {rat}");
    }
    if let Some(gran) = input.task_granularity {
        let _ = writeln!(md, "- Task Granularity: `{}`", granularity_label(gran));
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

    let _ = writeln!(md, "## Approved Spec Summary");
    if input.spec_brief.trim().is_empty() {
        let _ = writeln!(md, "_No brief summary provided._\n");
    } else {
        let _ = writeln!(md, "{}\n", input.spec_brief.trim());
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
    append_plan_m2_context(&mut md, input);
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

    let _ = writeln!(md, "## Approved Spec Deliverable (`spec.md`)");
    let _ = writeln!(md, "```markdown\n{}\n```\n", input.spec_markdown.trim_end());
    append_expected_contract(&mut md, &input.expected_output_contract);

    let brief_digest = sha256_prefixed(md.as_bytes());
    AssembledPlanBrief {
        markdown: md,
        brief_digest,
    }
}

fn append_expected_contract(md: &mut String, custom: &str) {
    let _ = writeln!(md, "## Expected Plan Output Contract");
    if !custom.trim().is_empty() {
        let _ = writeln!(md, "{}\n", custom.trim());
    }
    let _ = writeln!(
        md,
        "- Address all six design lenses (UX, functional, architecture, quality, infrastructure, delivery) with explicit applicability decisions."
    );
    let _ = writeln!(
        md,
        "- Compare two materially different design alternatives for applicable stages and record selection rationale."
    );
    let _ = writeln!(
        md,
        "- Produce a task DAG with strategy, granularity, exclusive file ownership (`filesAllowed` / `filesForbidden`), planned checks (argv-only), and criterion-to-evidence mappings for every `AC-*`."
    );
    let _ = writeln!(
        md,
        "- Do NOT run `git commit` or auto-commit deliverables under `.legion/workflows/`."
    );
}
