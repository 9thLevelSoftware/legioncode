#![forbid(unsafe_code)]
//! Spec intake (`briefPath`, `inputPaths`), deterministic brownfield workspace scan, and initial exploration seeds.

use std::fs;
use std::path::Path;

use legion_contracts::workflow_board::{
    WorkflowBoardAssistanceMode, WorkflowBoardIntakeGoal, WorkflowBoardRequestError,
    WorkflowBoardSourceInputDigest, WorkflowBoardSpecIntakeAnalysis, WorkflowBoardSpecSourceType,
};
use legion_workflows::sha256_prefixed;

use crate::workflow_board_fs::read_confined_input_file;

pub(crate) struct SeedDecisionData {
    pub decision_id: String,
    pub prompt: String,
    pub recommendation: Option<String>,
    pub rationale: Option<String>,
    pub depends_on: Vec<String>,
}

pub(crate) struct PreparedIntake {
    pub brief: String,
    pub brief_digest: Option<String>,
    pub source_digest: Option<String>,
    pub source_input_digests: Vec<WorkflowBoardSourceInputDigest>,
    pub intake_analysis: Option<WorkflowBoardSpecIntakeAnalysis>,
    pub source_refs: Vec<String>,
}

const BROWNFIELD_PROBES: &[&str] = &[
    "Cargo.toml",
    "package.json",
    "pyproject.toml",
    "go.mod",
    "README.md",
    "AGENTS.md",
];

pub(crate) struct IntakeRequest<'a> {
    pub title: &'a str,
    pub explicit_brief: Option<&'a str>,
    pub brief_path: Option<&'a str>,
    pub input_paths: Option<&'a [String]>,
    pub source_type: WorkflowBoardSpecSourceType,
    pub intake_goal: Option<WorkflowBoardIntakeGoal>,
    pub assistance_mode: WorkflowBoardAssistanceMode,
}

pub(crate) fn prepare_intake(
    workspace_root: &Path,
    req: &IntakeRequest<'_>,
) -> Result<PreparedIntake, WorkflowBoardRequestError> {
    let IntakeRequest {
        title,
        explicit_brief,
        brief_path,
        input_paths,
        source_type,
        intake_goal,
        assistance_mode,
    } = *req;
    let mut source_input_digests = Vec::new();
    let mut source_refs = Vec::new();
    let mut supplied_decisions = Vec::new();
    let mut inferred_decisions = Vec::new();

    let (brief_from_file, brief_digest) = match brief_path {
        Some(path) => {
            let (content, digest) = read_confined_input_file(workspace_root, path)?;
            source_refs.push(path.trim().to_owned());
            source_input_digests.push(WorkflowBoardSourceInputDigest {
                path: path.trim().to_owned(),
                digest: digest.clone(),
            });
            extract_headings_as_supplied(&content, &mut supplied_decisions);
            (Some(content), Some(digest))
        }
        None => (None, None),
    };

    if let Some(paths) = input_paths {
        for raw_path in paths {
            let trimmed = raw_path.trim();
            if trimmed.is_empty() {
                continue;
            }
            let (content, digest) = read_confined_input_file(workspace_root, trimmed)?;
            if !source_refs.iter().any(|r| r == trimmed) {
                source_refs.push(trimmed.to_owned());
            }
            if !source_input_digests.iter().any(|d| d.path == trimmed) {
                source_input_digests.push(WorkflowBoardSourceInputDigest {
                    path: trimmed.to_owned(),
                    digest,
                });
            }
            extract_headings_as_supplied(&content, &mut supplied_decisions);
        }
    }

    let brief = match explicit_brief {
        Some(b) if !b.trim().is_empty() => b.to_owned(),
        _ => brief_from_file.unwrap_or_default(),
    };
    if !brief.trim().is_empty() && supplied_decisions.is_empty() {
        supplied_decisions.push(format!("Core goal provided for `{}`", title.trim()));
    }

    let scan_brownfield = source_type == WorkflowBoardSpecSourceType::Codebase
        || intake_goal == Some(WorkflowBoardIntakeGoal::Change)
        || intake_goal == Some(WorkflowBoardIntakeGoal::Audit)
        || !source_input_digests.is_empty();
    if scan_brownfield {
        scan_workspace_manifest(workspace_root, &mut inferred_decisions, &mut source_refs);
    }

    let missing_decisions =
        compute_missing_decisions(&brief, intake_goal, assistance_mode, &supplied_decisions);
    let source_digest = if source_input_digests.is_empty() {
        None
    } else {
        let joined = source_input_digests
            .iter()
            .map(|d| format!("{}:{}", d.path, d.digest))
            .collect::<Vec<_>>()
            .join("\n");
        Some(sha256_prefixed(joined.as_bytes()))
    };

    let intake_analysis = if scan_brownfield || brief_digest.is_some() {
        Some(WorkflowBoardSpecIntakeAnalysis {
            supplied_decisions,
            inferred_decisions,
            missing_decisions,
            source_digest: source_digest.clone(),
        })
    } else {
        None
    };

    Ok(PreparedIntake {
        brief,
        brief_digest,
        source_digest,
        source_input_digests,
        intake_analysis,
        source_refs,
    })
}

fn extract_headings_as_supplied(content: &str, out: &mut Vec<String>) {
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(heading) = trimmed.strip_prefix('#') {
            let clean = heading.trim_start_matches('#').trim();
            if !clean.is_empty() && out.len() < 8 && !out.iter().any(|x| x == clean) {
                out.push(format!("Section: {clean}"));
            }
        }
    }
}

fn scan_workspace_manifest(
    workspace_root: &Path,
    inferred: &mut Vec<String>,
    source_refs: &mut Vec<String>,
) {
    for rel in BROWNFIELD_PROBES {
        let full = workspace_root.join(rel);
        if full.is_file()
            && let Ok(text) = fs::read_to_string(&full)
        {
            if !source_refs.iter().any(|r| r == rel) {
                source_refs.push((*rel).to_owned());
            }
            let summary = match *rel {
                "Cargo.toml" => format!(
                    "Detected Rust workspace manifest (`Cargo.toml`, {} lines)",
                    text.lines().count()
                ),
                "package.json" => format!(
                    "Detected Node/TypeScript manifest (`package.json`, {} lines)",
                    text.lines().count()
                ),
                other => format!("Detected workspace reference `{other}`"),
            };
            if !inferred.iter().any(|x| x == &summary) {
                inferred.push(summary);
            }
        }
    }
}

fn compute_missing_decisions(
    brief: &str,
    goal: Option<WorkflowBoardIntakeGoal>,
    mode: WorkflowBoardAssistanceMode,
    supplied: &[String],
) -> Vec<String> {
    let mut missing = Vec::new();
    let lower = brief.to_ascii_lowercase();
    if !lower.contains("acceptance") && !lower.contains("ac-") {
        missing.push("Define measurable acceptance criteria (`AC-*`)".to_owned());
    }
    if !lower.contains("rollback") && !lower.contains("compat") {
        missing.push("Clarify compatibility and rollback expectations".to_owned());
    }
    if goal == Some(WorkflowBoardIntakeGoal::Change) && !lower.contains("scope") {
        missing.push("Confirm brownfield module boundaries and non-goals".to_owned());
    }
    if mode == WorkflowBoardAssistanceMode::Guided && supplied.is_empty() {
        missing.push("Specify primary user workflow and failure handling".to_owned());
    }
    missing
}

/// Builds deterministic dependency-linked exploration seeds when starting `exploreStep` on a fresh card.
pub(crate) fn build_exploration_seeds(
    title: &str,
    intake_goal: Option<&str>,
    missing_decisions: &[String],
) -> Vec<SeedDecisionData> {
    let goal_label = intake_goal.unwrap_or("feature");
    let first_prompt = missing_decisions
        .first()
        .cloned()
        .unwrap_or_else(|| format!("Confirm primary scope & acceptance boundary for `{title}`"));
    let second_prompt = missing_decisions
        .get(1)
        .cloned()
        .unwrap_or_else(|| format!("Confirm compatibility & rollback strategy for `{goal_label}`"));
    let third_prompt = missing_decisions.get(2).cloned().unwrap_or_else(|| {
        "Confirm verification checks & non-UI wireframe applicability".to_owned()
    });

    vec![
        SeedDecisionData {
            decision_id: "DEC-1".to_owned(),
            prompt: first_prompt,
            recommendation: Some(
                "Anchor scope to explicit AC-* criteria with minimal surface area.".to_owned(),
            ),
            rationale: Some(
                "Establishing measurable criteria first prevents downstream plan churn.".to_owned(),
            ),
            depends_on: Vec::new(),
        },
        SeedDecisionData {
            decision_id: "DEC-2".to_owned(),
            prompt: second_prompt,
            recommendation: Some(
                "Preserve backward compatibility and keep state mutations transactional."
                    .to_owned(),
            ),
            rationale: Some(
                "Transactional CAS updates allow safe return-for-revision without data loss."
                    .to_owned(),
            ),
            depends_on: Vec::new(),
        },
        SeedDecisionData {
            decision_id: "DEC-3".to_owned(),
            prompt: third_prompt,
            recommendation: Some(
                "Use argv-only verification checks and keep wireframes opt-in.".to_owned(),
            ),
            rationale: Some(
                "Argv-only checks prevent shell injection and non-UI cards skip wireframes."
                    .to_owned(),
            ),
            depends_on: Vec::new(),
        },
    ]
}
