#![forbid(unsafe_code)]
//! Plan payload merging and relational write-journal persistence helpers.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use legion_contracts::orchestration::PatchValue;
use legion_contracts::workflow_board::{WorkflowBoardRequestError, WorkflowBoardUpdatePlanInput};
use legion_persistence::workflow_board_repo::types_m2::{
    AcMappingInput, AlternativeInput, AlternativeSelectionUpdate, CardPlanningDetail, LensInput,
    PlannedCheckInput, TaskContractInput, TaskContractRow, UpdatePlanInput,
};
use legion_persistence::workflow_board_repo::update_plan;
use uuid::Uuid;

use crate::workflow_board_fs::is_symlink_or_reparse_point;
use crate::workflow_board_journal::{JournaledWriteSpec, execute_journaled_writes};
use crate::workflow_board_types::{enum_as_str, internal, map_repo_err};

type LensRowTuple = (String, Option<bool>, Option<String>, Option<String>);
type TaskSliceTuple<'a> = (
    Vec<&'a str>,
    Vec<&'a str>,
    Vec<&'a str>,
    Vec<&'a str>,
    Vec<&'a str>,
);

struct OwnedTask {
    task_id: String,
    title: String,
    deps: Vec<String>,
    blocks: Vec<String>,
    files_allowed: Vec<String>,
    files_forbidden: Vec<String>,
    expected_artifacts: Vec<String>,
    verification_commands: Vec<Vec<String>>,
    testing_method: String,
    testing_limitations: Option<String>,
    status: Option<String>,
}

struct OwnedCheck {
    check_id: String,
    owner_task_id: Option<String>,
    argv: Vec<String>,
    label: Option<String>,
}

struct OwnedAcMapping {
    criterion_id: String,
    method: String,
    check_ids: Vec<String>,
    command_argv: Option<Vec<String>>,
    procedure: Option<String>,
    evidence_ref: Option<String>,
    external_source: Option<String>,
}

struct OwnedPlanInputs {
    lenses: Option<Vec<LensRowTuple>>,
    tasks: Option<Vec<OwnedTask>>,
    checks: Option<Vec<OwnedCheck>>,
    ac_mappings: Option<Vec<OwnedAcMapping>>,
}

fn prepare_owned_plan_inputs(input: &WorkflowBoardUpdatePlanInput) -> OwnedPlanInputs {
    let lenses = input.lenses.as_ref().map(|ls| {
        ls.iter()
            .map(|l| {
                (
                    enum_as_str(&l.lens).unwrap_or_default(),
                    l.applicable,
                    l.summary.clone(),
                    l.reason.clone(),
                )
            })
            .collect()
    });
    let tasks = input.tasks.as_ref().map(|ts| {
        ts.iter()
            .map(|t| {
                let mut deps = Vec::new();
                for d in t
                    .depends_on
                    .iter()
                    .flatten()
                    .chain(t.blocked_by.iter().flatten())
                {
                    let tr = d.trim();
                    if !tr.is_empty() && !deps.iter().any(|x| x == tr) {
                        deps.push(tr.to_owned());
                    }
                }
                OwnedTask {
                    task_id: t.task_id.clone(),
                    title: t.title.clone(),
                    deps,
                    blocks: t.blocks.clone().unwrap_or_default(),
                    files_allowed: t.files_allowed.clone(),
                    files_forbidden: t.files_forbidden.clone().unwrap_or_default(),
                    expected_artifacts: t.expected_artifacts.clone().unwrap_or_default(),
                    verification_commands: t.verification_commands.clone().unwrap_or_default(),
                    testing_method: enum_as_str(&t.testing_method).unwrap_or_default(),
                    testing_limitations: t.testing_limitations.clone(),
                    status: t.status.and_then(|s| enum_as_str(&s)),
                }
            })
            .collect()
    });
    let checks = input.checks.as_ref().map(|cs| {
        cs.iter()
            .map(|c| OwnedCheck {
                check_id: c.id.clone(),
                owner_task_id: c.criterion_id.clone(),
                argv: c.argv.clone(),
                label: c.description.clone(),
            })
            .collect()
    });
    let ac_mappings = input.ac_mappings.as_ref().map(|ms| {
        ms.iter()
            .map(|m| OwnedAcMapping {
                criterion_id: m.criterion_id.clone(),
                method: enum_as_str(&m.method).unwrap_or_default(),
                check_ids: m.check_ids.clone().unwrap_or_default(),
                command_argv: m.command.clone(),
                procedure: m.procedure.clone(),
                evidence_ref: m.external_ref.clone(),
                external_source: m.external_source.clone(),
            })
            .collect()
    });
    OwnedPlanInputs {
        lenses,
        tasks,
        checks,
        ac_mappings,
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "Coordinates journaled plan/design/task file writes with relational SQLite update_plan"
)]
pub(crate) fn persist_plan_with_journal(
    conn: &rusqlite::Connection,
    card_id: &str,
    card_dir: &std::path::Path,
    writes: &[JournaledWriteSpec],
    expected_rev_i64: i64,
    input: &WorkflowBoardUpdatePlanInput,
    plan_json: &str,
    plan_digest: &str,
    design_digest: &str,
    now_iso: &str,
) -> Result<CardPlanningDetail, WorkflowBoardRequestError> {
    let owned = prepare_owned_plan_inputs(input);
    let strategy_str = input.strategy.and_then(|s| enum_as_str(&s));
    let strategy_meta_json = input
        .strategy_metadata
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|e| internal(e.to_string()))?;
    let granularity_str = input.granularity.and_then(|g| enum_as_str(&g));
    let sel_alt = match &input.selected_alternative_id {
        PatchValue::Absent => AlternativeSelectionUpdate::Keep,
        PatchValue::Null => AlternativeSelectionUpdate::Clear,
        PatchValue::Set(id) => AlternativeSelectionUpdate::Set(id.as_str()),
    };

    let rev_id = format!("rev-{}", Uuid::new_v4().simple());
    execute_journaled_writes(conn, card_id, card_dir, writes, now_iso, |tx| {
        run_update_plan_tx(
            tx,
            card_id,
            &rev_id,
            expected_rev_i64,
            input,
            &owned,
            sel_alt,
            strategy_str.as_deref(),
            strategy_meta_json.as_deref(),
            granularity_str.as_deref(),
            plan_json,
            plan_digest,
            design_digest,
            now_iso,
        )
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "Borrows owned plan slices into repository UpdatePlanInput inside WriteTx"
)]
fn run_update_plan_tx(
    tx: &rusqlite::Connection,
    card_id: &str,
    rev_id: &str,
    expected_rev_i64: i64,
    input: &WorkflowBoardUpdatePlanInput,
    owned: &OwnedPlanInputs,
    sel_alt: AlternativeSelectionUpdate<'_>,
    strategy_str: Option<&str>,
    strategy_meta_json: Option<&str>,
    granularity_str: Option<&str>,
    plan_json: &str,
    plan_digest: &str,
    design_digest: &str,
    now_iso: &str,
) -> Result<CardPlanningDetail, WorkflowBoardRequestError> {
    let lens_refs: Option<Vec<LensInput<'_>>> = owned.lenses.as_ref().map(|ls| {
        ls.iter()
            .map(|(l, app, sum, rsn)| LensInput {
                lens: l,
                applicable: *app,
                summary: sum.as_deref(),
                reason: rsn.as_deref(),
            })
            .collect()
    });
    let alt_refs: Option<Vec<AlternativeInput<'_>>> = input.alternatives.as_ref().map(|alts| {
        alts.iter()
            .map(|a| AlternativeInput {
                id: &a.id,
                title: &a.title,
                summary: &a.summary,
                tradeoffs: &a.tradeoffs,
            })
            .collect()
    });
    let task_slices: Option<Vec<TaskSliceTuple<'_>>> = owned.tasks.as_ref().map(|ts| {
        ts.iter()
            .map(|t| {
                (
                    t.deps.iter().map(String::as_str).collect(),
                    t.blocks.iter().map(String::as_str).collect(),
                    t.files_allowed.iter().map(String::as_str).collect(),
                    t.files_forbidden.iter().map(String::as_str).collect(),
                    t.expected_artifacts.iter().map(String::as_str).collect(),
                )
            })
            .collect()
    });
    let task_refs = build_task_contract_refs(owned.tasks.as_deref(), task_slices.as_deref());
    let check_refs: Option<Vec<PlannedCheckInput<'_>>> = owned.checks.as_ref().map(|cs| {
        cs.iter()
            .map(|c| PlannedCheckInput {
                check_id: &c.check_id,
                owner_task_id: c.owner_task_id.as_deref(),
                argv: &c.argv,
                label: c.label.as_deref(),
            })
            .collect()
    });
    let ac_slices: Option<Vec<Vec<&str>>> = owned.ac_mappings.as_ref().map(|ms| {
        ms.iter()
            .map(|m| m.check_ids.iter().map(String::as_str).collect())
            .collect()
    });
    let ac_refs: Option<Vec<AcMappingInput<'_>>> = owned
        .ac_mappings
        .as_ref()
        .zip(ac_slices.as_ref())
        .map(|(ms, sls)| {
            ms.iter()
                .zip(sls.iter())
                .map(|(m, cids)| AcMappingInput {
                    criterion_id: &m.criterion_id,
                    method: &m.method,
                    check_ids: cids.as_slice(),
                    command_argv: m.command_argv.as_deref(),
                    procedure: m.procedure.as_deref(),
                    evidence_ref: m.evidence_ref.as_deref(),
                    external_source: m.external_source.as_deref(),
                })
                .collect()
        });

    update_plan(
        tx,
        &UpdatePlanInput {
            card_id,
            revision_id: rev_id,
            expected_revision: expected_rev_i64,
            lenses: lens_refs.as_deref(),
            alternatives: alt_refs.as_deref(),
            selected_alternative_id: sel_alt,
            selection_rationale: input.selection_rationale.as_deref(),
            revise_alternatives_feedback: input.revise_alternatives_feedback.as_deref(),
            strategy: strategy_str,
            strategy_rationale: input.strategy_rationale.as_deref(),
            strategy_metadata_json: strategy_meta_json,
            granularity: granularity_str,
            tasks: task_refs.as_deref(),
            checks: check_refs.as_deref(),
            ac_mappings: ac_refs.as_deref(),
            plan_json: Some(plan_json),
            plan_digest: Some(plan_digest),
            design_digest: Some(design_digest),
            tasks_digest: None,
        },
        now_iso,
    )
    .map_err(map_repo_err)
}

fn build_task_contract_refs<'a>(
    tasks: Option<&'a [OwnedTask]>,
    slices: Option<&'a [TaskSliceTuple<'a>]>,
) -> Option<Vec<TaskContractInput<'a>>> {
    tasks.zip(slices).map(|(ts, sls)| {
        ts.iter()
            .zip(sls.iter())
            .map(|(t, (deps, blks, allowed, forb, exp))| TaskContractInput {
                task_id: &t.task_id,
                title: &t.title,
                depends_on: deps.as_slice(),
                blocks: blks.as_slice(),
                files_allowed: allowed.as_slice(),
                files_forbidden: forb.as_slice(),
                expected_artifacts: exp.as_slice(),
                verification_commands: &t.verification_commands,
                testing_method: &t.testing_method,
                testing_limitations: t.testing_limitations.as_deref(),
                status: t.status.as_deref(),
            })
            .collect()
    })
}

pub(crate) fn cleanup_obsolete_task_files(card_dir: &Path, active_tasks: &[TaskContractRow]) {
    let tasks_dir = card_dir.join("tasks");
    let Ok(dir_meta) = fs::symlink_metadata(&tasks_dir) else {
        return;
    };
    if is_symlink_or_reparse_point(&dir_meta) || !dir_meta.is_dir() {
        return;
    }
    let Ok(canon_card) = fs::canonicalize(card_dir) else {
        return;
    };
    let Ok(canon_tasks) = fs::canonicalize(&tasks_dir) else {
        return;
    };
    if canon_tasks.strip_prefix(&canon_card).is_err() {
        return;
    }
    let keep: HashSet<String> = active_tasks
        .iter()
        .map(|t| format!("{}.md", t.task_id))
        .collect();
    let Ok(entries) = fs::read_dir(&canon_tasks) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if is_symlink_or_reparse_point(&meta) || !meta.is_file() {
            continue;
        }
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.ends_with(".md") && !keep.contains(name_str.as_ref()) {
            let _ = fs::remove_file(&path);
        }
    }
}
