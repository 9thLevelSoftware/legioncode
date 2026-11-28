#![forbid(unsafe_code)]
//! Deterministic Execute and Independent Review stage brief assembly and SHA-256 digests.

use std::fmt::Write as _;

use legion_contracts::workflow_board::{
    WorkflowBoardAcEvidenceMapping, WorkflowBoardAcceptanceCriterion, WorkflowBoardPlannedCheck,
    WorkflowBoardTaskContract,
};

use crate::stage_brief::sha256_prefixed;
use crate::stage_brief_render::{
    append_ac_section, append_code_list_section, append_list_section, opt_non_empty,
};

/// Pure inputs required to assemble a deterministic Execute task brief.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecuteBriefInput {
    pub card_id: String,
    pub card_title: String,
    pub project_id: String,
    pub workspace_root: String,
    pub worktree_path: Option<String>,
    pub branch: Option<String>,
    pub card_revision: u64,
    pub spec_digest: String,
    pub plan_digest: String,
    pub product_snapshot_digest: Option<String>,
    pub task: WorkflowBoardTaskContract,
    pub completed_task_ids: Vec<String>,
    pub planned_checks: Vec<WorkflowBoardPlannedCheck>,
    pub acceptance_criteria: Vec<WorkflowBoardAcceptanceCriterion>,
    pub constraints: Vec<String>,
    pub source_refs: Vec<String>,
    pub expected_output_contract: String,
}

/// Rendered Execute brief markdown and its `"sha256:{hex}"` content digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssembledExecuteBrief {
    pub markdown: String,
    pub brief_digest: String,
}

/// Pure inputs required to assemble a deterministic Independent Review brief.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewBriefInput {
    pub card_id: String,
    pub card_title: String,
    pub project_id: String,
    pub workspace_root: String,
    pub worktree_path: Option<String>,
    pub branch: Option<String>,
    pub card_revision: u64,
    pub spec_digest: String,
    pub plan_digest: String,
    pub product_snapshot_digest: String,
    pub spec_summary: String,
    pub acceptance_criteria: Vec<WorkflowBoardAcceptanceCriterion>,
    pub ac_mappings: Vec<WorkflowBoardAcEvidenceMapping>,
    pub task_summaries: Vec<String>,
    pub check_receipt_summaries: Vec<String>,
    pub changed_files_summary: Vec<String>,
    pub diff_excerpt: String,
    pub expected_output_contract: String,
}

/// Rendered Independent Review brief markdown and its `"sha256:{hex}"` content digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssembledReviewBrief {
    pub markdown: String,
    pub brief_digest: String,
}

fn append_planned_checks_section(md: &mut String, checks: &[WorkflowBoardPlannedCheck]) {
    let _ = writeln!(md, "## Planned Checks");
    if checks.is_empty() {
        let _ = writeln!(md, "- None.\n");
        return;
    }
    for chk in checks {
        let desc = opt_non_empty(chk.description.as_deref(), "(no description)");
        let _ = writeln!(
            md,
            "- `{}`: `{}` — {desc}",
            chk.id.trim(),
            chk.argv.join(" ")
        );
    }
    let _ = writeln!(md);
}

/// Deterministically builds the Execute task brief markdown and SHA-256 digest.
#[must_use]
pub fn build_execute_brief(input: &ExecuteBriefInput) -> AssembledExecuteBrief {
    let mut md = String::new();
    let _ = writeln!(
        md,
        "# Execute Task Brief: {} (`{}`)\n",
        input.card_title.trim(),
        input.task.task_id.trim()
    );
    let _ = writeln!(md, "## Metadata");
    let _ = writeln!(md, "- Card ID: `{}`", input.card_id.trim());
    let _ = writeln!(md, "- Project ID: `{}`", input.project_id.trim());
    let _ = writeln!(md, "- Card Revision: `{}`", input.card_revision);
    let _ = writeln!(md, "- Approved Spec Digest: `{}`", input.spec_digest.trim());
    let _ = writeln!(md, "- Approved Plan Digest: `{}`", input.plan_digest.trim());
    let _ = writeln!(
        md,
        "- Product Snapshot Digest: `{}`",
        opt_non_empty(input.product_snapshot_digest.as_deref(), "(initial)")
    );
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

    let _ = writeln!(md, "## Assigned Task Contract");
    let _ = writeln!(md, "- Task ID: `{}`", input.task.task_id.trim());
    let _ = writeln!(md, "- Title: {}", input.task.title.trim());
    let _ = writeln!(md, "- Testing Method: `{:?}`\n", input.task.testing_method);

    append_code_list_section(
        &mut md,
        "Allowed Files (`filesAllowed`)",
        &input.task.files_allowed,
        "- None.",
    );
    let forbidden = input.task.files_forbidden.as_deref().unwrap_or(&[]);
    append_code_list_section(
        &mut md,
        "Forbidden Files (`filesForbidden`)",
        forbidden,
        "- None.",
    );
    let expected_artifacts = input.task.expected_artifacts.as_deref().unwrap_or(&[]);
    append_code_list_section(&mut md, "Expected Artifacts", expected_artifacts, "- None.");
    append_code_list_section(
        &mut md,
        "Upstream Completed Tasks",
        &input.completed_task_ids,
        "- None.",
    );

    append_planned_checks_section(&mut md, &input.planned_checks);
    append_ac_section(&mut md, &input.acceptance_criteria);
    append_list_section(
        &mut md,
        "Constraints",
        &input.constraints,
        "- None specified.",
    );
    append_code_list_section(&mut md, "Source References", &input.source_refs, "- None.");

    let _ = writeln!(md, "## Expected Execute Output Contract");
    if !input.expected_output_contract.trim().is_empty() {
        let _ = writeln!(md, "{}\n", input.expected_output_contract.trim());
    }
    let _ = writeln!(
        md,
        "- Modify ONLY files permitted by `filesAllowed` and never touch `filesForbidden`."
    );
    let _ = writeln!(
        md,
        "- Unexpected edits will be preserved for review and will block progression until resolved or amended."
    );
    let _ = writeln!(
        md,
        "- Use `legion_workflow_board_propose` (`stage: \"execute\"`, `kind: \"check\"` or `\"execute\"`) to run planned checks or submit task completion."
    );
    let _ = writeln!(
        md,
        "- Do NOT run `git commit` or auto-commit deliverables under `.legion/workflows/`."
    );

    let brief_digest = sha256_prefixed(md.as_bytes());
    AssembledExecuteBrief {
        markdown: md,
        brief_digest,
    }
}

/// Deterministically builds the Independent Review brief markdown and SHA-256 digest.
#[must_use]
pub fn build_review_brief(input: &ReviewBriefInput) -> AssembledReviewBrief {
    let mut md = String::new();
    let _ = writeln!(
        md,
        "# Independent Review Brief: {}\n",
        input.card_title.trim()
    );
    let _ = writeln!(md, "## Metadata");
    let _ = writeln!(md, "- Card ID: `{}`", input.card_id.trim());
    let _ = writeln!(md, "- Project ID: `{}`", input.project_id.trim());
    let _ = writeln!(md, "- Card Revision: `{}`", input.card_revision);
    let _ = writeln!(md, "- Approved Spec Digest: `{}`", input.spec_digest.trim());
    let _ = writeln!(md, "- Approved Plan Digest: `{}`", input.plan_digest.trim());
    let _ = writeln!(
        md,
        "- Product Snapshot Digest: `{}`",
        input.product_snapshot_digest.trim()
    );
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
    if input.spec_summary.trim().is_empty() {
        let _ = writeln!(md, "_No spec summary provided._\n");
    } else {
        let _ = writeln!(md, "{}\n", input.spec_summary.trim());
    }

    append_ac_section(&mut md, &input.acceptance_criteria);

    let ac_mapping_lines: Vec<String> = input
        .ac_mappings
        .iter()
        .map(|m| {
            format!(
                "{}: method={:?}, checks={:?}",
                m.criterion_id.trim(),
                m.method,
                m.check_ids.as_deref().unwrap_or(&[])
            )
        })
        .collect();
    append_list_section(
        &mut md,
        "AC Evidence Mappings",
        &ac_mapping_lines,
        "- None.",
    );
    append_list_section(
        &mut md,
        "Task Execution Summaries",
        &input.task_summaries,
        "- None.",
    );
    append_list_section(
        &mut md,
        "Snapshot-Bound Check Receipts",
        &input.check_receipt_summaries,
        "- None.",
    );
    append_code_list_section(
        &mut md,
        "Changed Files Under Review",
        &input.changed_files_summary,
        "- None.",
    );

    let _ = writeln!(md, "## Worktree Diff Excerpt");
    if input.diff_excerpt.trim().is_empty() {
        let _ = writeln!(md, "_No diff excerpt._\n");
    } else {
        let _ = writeln!(md, "```diff\n{}\n```\n", input.diff_excerpt.trim_end());
    }

    let _ = writeln!(md, "## Expected Review Output Contract");
    if !input.expected_output_contract.trim().is_empty() {
        let _ = writeln!(md, "{}\n", input.expected_output_contract.trim());
    }
    let _ = writeln!(
        md,
        "- Review independently from the approved spec, plan, checks, and diff without modifying product files."
    );
    let _ = writeln!(
        md,
        "- Submit structured output via `legion_workflow_board_propose` (`stage: \"review\"`, `kind: \"review\"`, `verdict: \"pass\" | \"findings\" | \"failed\"`, `summary`, `findings: [{{ id, severity, criterionId, file, line, message }}]`)."
    );
    let _ = writeln!(
        md,
        "- Findings must keep `disposition: \"unresolved\"`; only human operators may resolve findings or approve delivery."
    );

    let brief_digest = sha256_prefixed(md.as_bytes());
    AssembledReviewBrief {
        markdown: md,
        brief_digest,
    }
}
