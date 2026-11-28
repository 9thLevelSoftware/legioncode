//! Domain validation for M2 Plan authoring, strategy invariants, task DAGs, and checks.

use std::collections::{HashMap, HashSet, VecDeque};

use legion_contracts::workflow_board::m2_validators::is_git_metadata_segment;
use legion_contracts::workflow_board::{is_valid_argv_command, is_valid_artifact_key};
use serde_json::Value;

use super::WorkflowBoardRepoError;
use super::types_m2::{
    AcMappingInput, AlternativeInput, LensInput, PlannedCheckInput, PlannedCheckRow,
    TaskContractInput,
};

pub(crate) const ALL_SIX_LENSES: &[&str] = &[
    "ux",
    "functional",
    "architecture",
    "quality",
    "infrastructure",
    "delivery",
];

pub(crate) fn validate_lenses(lenses: &[LensInput<'_>]) -> Result<(), WorkflowBoardRepoError> {
    let mut seen = HashSet::new();
    for entry in lenses {
        let lens = entry.lens.trim();
        if !ALL_SIX_LENSES.contains(&lens) {
            return Err(WorkflowBoardRepoError::InvalidInput(format!(
                "unknown design lens `{lens}`"
            )));
        }
        if !seen.insert(lens) {
            return Err(WorkflowBoardRepoError::InvalidInput(format!(
                "duplicate design lens `{lens}`"
            )));
        }
        match entry.applicable {
            Some(true) if entry.summary.map(str::trim).is_none_or(str::is_empty) => {
                return Err(WorkflowBoardRepoError::InvalidInput(format!(
                    "applicable lens `{lens}` requires non-empty summary"
                )));
            }
            Some(false) if entry.reason.map(str::trim).is_none_or(str::is_empty) => {
                return Err(WorkflowBoardRepoError::InvalidInput(format!(
                    "non-applicable lens `{lens}` requires non-empty reason"
                )));
            }
            Some(_) | None => {}
        }
    }
    Ok(())
}

pub(crate) fn validate_alternatives(
    alts: &[AlternativeInput<'_>],
) -> Result<(), WorkflowBoardRepoError> {
    if alts.len() != 2 {
        return Err(WorkflowBoardRepoError::InvalidInput(
            "expected exactly 2 design alternatives".to_owned(),
        ));
    }
    let a = &alts[0];
    let b = &alts[1];
    if a.id.trim().is_empty() || b.id.trim().is_empty() || a.id.trim() == b.id.trim() {
        return Err(WorkflowBoardRepoError::InvalidInput(
            "design alternatives must have distinct non-empty IDs".to_owned(),
        ));
    }
    if a.title.trim().is_empty()
        || b.title.trim().is_empty()
        || a.summary.trim().is_empty()
        || b.summary.trim().is_empty()
        || a.tradeoffs.trim().is_empty()
        || b.tradeoffs.trim().is_empty()
    {
        return Err(WorkflowBoardRepoError::InvalidInput(
            "design alternative title, summary, and tradeoffs cannot be empty".to_owned(),
        ));
    }
    if a.title.trim() == b.title.trim()
        && a.summary.trim() == b.summary.trim()
        && a.tradeoffs.trim() == b.tradeoffs.trim()
    {
        return Err(WorkflowBoardRepoError::InvalidInput(
            "design alternatives must be materially distinct".to_owned(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_granularity(granularity: &str) -> Result<(), WorkflowBoardRepoError> {
    let g = granularity.trim();
    if !matches!(g, "coarse" | "balanced" | "fine") {
        return Err(WorkflowBoardRepoError::InvalidInput(format!(
            "unsupported task granularity `{g}`"
        )));
    }
    Ok(())
}

pub(crate) fn validate_strategy(
    strategy: &str,
    rationale: Option<&str>,
    meta_json: Option<&str>,
) -> Result<(), WorkflowBoardRepoError> {
    let s = strategy.trim();
    if !matches!(s, "outcomes" | "risk-first" | "expand-contract" | "custom") {
        return Err(WorkflowBoardRepoError::InvalidInput(format!(
            "unsupported planning strategy `{s}`"
        )));
    }
    if s == "custom" && rationale.map(str::trim).is_none_or(str::is_empty) {
        return Err(WorkflowBoardRepoError::InvalidInput(
            "custom strategy requires non-empty strategy_rationale".to_owned(),
        ));
    }
    if s == "risk-first" || s == "expand-contract" {
        let meta: Value = meta_json
            .map(serde_json::from_str)
            .transpose()
            .map_err(|source| WorkflowBoardRepoError::Json {
                operation: "parse_strategy_metadata",
                source,
            })?
            .unwrap_or(Value::Null);
        let non_empty = |k: &str| {
            meta.get(k)
                .and_then(Value::as_str)
                .map(str::trim)
                .is_some_and(|v| !v.is_empty())
        };
        if s == "risk-first" && !(non_empty("uncertainty") && non_empty("probeTaskId")) {
            return Err(WorkflowBoardRepoError::InvalidInput(
                "risk-first strategy requires uncertainty and probeTaskId".to_owned(),
            ));
        }
        if s == "expand-contract"
            && !(non_empty("compatibilityWindow")
                && non_empty("transitionStep")
                && non_empty("retirementPrereq"))
        {
            return Err(WorkflowBoardRepoError::InvalidInput(
                "expand-contract strategy requires compatibilityWindow, transitionStep, and retirementPrereq"
                    .to_owned(),
            ));
        }
    }
    Ok(())
}

fn validate_task_contract_fields<'a>(
    task: &TaskContractInput<'a>,
    ids: &mut HashSet<&'a str>,
    all_allowed: &mut HashSet<&'a str>,
) -> Result<(), WorkflowBoardRepoError> {
    let tid = task.task_id.trim();
    if tid.is_empty() || !ids.insert(tid) {
        return Err(WorkflowBoardRepoError::InvalidInput(format!(
            "duplicate or empty task_id `{tid}`"
        )));
    }
    if task.title.trim().is_empty() {
        return Err(WorkflowBoardRepoError::InvalidInput(format!(
            "task `{tid}` title cannot be empty"
        )));
    }
    if !matches!(
        task.testing_method.trim(),
        "test-first" | "regression-first" | "existing-checks"
    ) {
        return Err(WorkflowBoardRepoError::InvalidInput(format!(
            "unsupported testing_method `{}` on `{tid}`",
            task.testing_method
        )));
    }
    if task.files_allowed.is_empty() {
        return Err(WorkflowBoardRepoError::InvalidInput(format!(
            "task `{tid}` must declare non-empty files_allowed"
        )));
    }
    let mut forbidden_set = HashSet::new();
    for f in task.files_forbidden {
        let norm = f.trim();
        if !is_valid_artifact_key(norm) || norm.split('/').any(is_git_metadata_segment) {
            return Err(WorkflowBoardRepoError::InvalidInput(format!(
                "unsafe files_forbidden path `{f}` on `{tid}`"
            )));
        }
        forbidden_set.insert(norm);
    }
    for a in task.files_allowed {
        let norm = a.trim();
        if !is_valid_artifact_key(norm) || norm.split('/').any(is_git_metadata_segment) {
            return Err(WorkflowBoardRepoError::InvalidInput(format!(
                "unsafe files_allowed path `{a}` on `{tid}`"
            )));
        }
        if forbidden_set.contains(norm) {
            return Err(WorkflowBoardRepoError::InvalidInput(format!(
                "task `{tid}` has `{norm}` in both files_allowed and files_forbidden"
            )));
        }
        if !all_allowed.insert(norm) {
            return Err(WorkflowBoardRepoError::InvalidInput(format!(
                "path `{norm}` is claimed by multiple tasks' files_allowed"
            )));
        }
    }
    for cmd in task.verification_commands {
        if !is_valid_argv_command(cmd) {
            return Err(WorkflowBoardRepoError::InvalidInput(format!(
                "unsafe verification command on task `{tid}`"
            )));
        }
    }
    Ok(())
}

fn validate_task_dag(
    tasks: &[TaskContractInput<'_>],
    ids: &HashSet<&str>,
) -> Result<(), WorkflowBoardRepoError> {
    let mut dependents: HashMap<&str, Vec<&str>> = ids.iter().map(|&id| (id, Vec::new())).collect();
    let mut in_degree: HashMap<&str, usize> = ids.iter().map(|&id| (id, 0)).collect();
    let mut edges: HashSet<(&str, &str)> = HashSet::new();
    for task in tasks {
        let tid = task.task_id.trim();
        for &dep in task.depends_on {
            let d = dep.trim();
            if !ids.contains(d) || d == tid {
                return Err(WorkflowBoardRepoError::InvalidInput(format!(
                    "task `{tid}` references unknown or self dependency `{d}`"
                )));
            }
            if edges.insert((d, tid)) {
                if let Some(list) = dependents.get_mut(d) {
                    list.push(tid);
                }
                if let Some(deg) = in_degree.get_mut(tid) {
                    *deg += 1;
                }
            }
        }
        for &blk in task.blocks {
            let b = blk.trim();
            if !ids.contains(b) || b == tid {
                return Err(WorkflowBoardRepoError::InvalidInput(format!(
                    "task `{tid}` references unknown or self block target `{b}`"
                )));
            }
            if edges.insert((tid, b)) {
                if let Some(list) = dependents.get_mut(tid) {
                    list.push(b);
                }
                if let Some(deg) = in_degree.get_mut(b) {
                    *deg += 1;
                }
            }
        }
    }

    let mut queue: VecDeque<&str> = in_degree
        .iter()
        .filter_map(|(&id, &deg)| if deg == 0 { Some(id) } else { None })
        .collect();
    let mut visited = 0_usize;
    while let Some(cur) = queue.pop_front() {
        visited += 1;
        if let Some(children) = dependents.get(cur) {
            for &child in children {
                if let Some(deg) = in_degree.get_mut(child) {
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push_back(child);
                    }
                }
            }
        }
    }
    if visited != ids.len() {
        return Err(WorkflowBoardRepoError::InvalidInput(
            "task dependency graph contains a cycle".to_owned(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_tasks_and_dag(
    tasks: &[TaskContractInput<'_>],
) -> Result<(), WorkflowBoardRepoError> {
    let mut ids: HashSet<&str> = HashSet::new();
    let mut all_allowed: HashSet<&str> = HashSet::new();
    for task in tasks {
        validate_task_contract_fields(task, &mut ids, &mut all_allowed)?;
    }
    validate_task_dag(tasks, &ids)
}

pub(crate) fn validate_checks_and_ac_mappings(
    checks: Option<&[PlannedCheckInput<'_>]>,
    mappings: Option<&[AcMappingInput<'_>]>,
    spec_ac_ids: &[String],
    persisted_checks: &[PlannedCheckRow],
) -> Result<(), WorkflowBoardRepoError> {
    if let Some(check_list) = checks {
        let mut seen_checks = HashSet::new();
        for ch in check_list {
            let check_id = ch.check_id.trim();
            if check_id.is_empty() || !is_valid_argv_command(ch.argv) {
                return Err(WorkflowBoardRepoError::InvalidInput(format!(
                    "planned check `{}` must have non-empty id and argv-only command",
                    ch.check_id
                )));
            }
            if !seen_checks.insert(check_id) {
                return Err(WorkflowBoardRepoError::InvalidInput(format!(
                    "duplicate planned check_id `{check_id}`"
                )));
            }
        }
    }
    let effective_check_ids: HashSet<&str> = match checks {
        Some(check_list) => check_list.iter().map(|c| c.check_id.trim()).collect(),
        None => persisted_checks.iter().map(|c| c.check_id.trim()).collect(),
    };
    if let Some(map_list) = mappings {
        let known_ac: HashSet<&str> = spec_ac_ids.iter().map(String::as_str).collect();
        let mut seen = HashSet::new();
        for m in map_list {
            let cid = m.criterion_id.trim();
            if cid.is_empty() || !seen.insert(cid) {
                return Err(WorkflowBoardRepoError::InvalidInput(format!(
                    "duplicate or empty criterion_id `{cid}`"
                )));
            }
            if !known_ac.is_empty() && !known_ac.contains(cid) {
                return Err(WorkflowBoardRepoError::InvalidInput(format!(
                    "unknown acceptance criterion `{cid}`"
                )));
            }
            match m.method.trim() {
                "automated" => {
                    let valid_cmd = m.command_argv.is_some_and(is_valid_argv_command);
                    if !valid_cmd && m.check_ids.is_empty() {
                        return Err(WorkflowBoardRepoError::InvalidInput(format!(
                            "automated AC `{cid}` requires argv command or check_ids"
                        )));
                    }
                    if let Some(argv) = m.command_argv
                        && !is_valid_argv_command(argv)
                    {
                        return Err(WorkflowBoardRepoError::InvalidInput(format!(
                            "unsafe shell wrapper/operator in command for `{cid}`"
                        )));
                    }
                    for &check_ref in m.check_ids {
                        let ref_id = check_ref.trim();
                        if ref_id.is_empty() || !effective_check_ids.contains(ref_id) {
                            return Err(WorkflowBoardRepoError::InvalidInput(format!(
                                "automated AC `{cid}` references unknown check_id `{ref_id}`"
                            )));
                        }
                    }
                }
                "manual" => {
                    if m.procedure.map(str::trim).is_none_or(str::is_empty) {
                        return Err(WorkflowBoardRepoError::InvalidInput(format!(
                            "manual AC `{cid}` requires concrete non-empty procedure"
                        )));
                    }
                }
                "external" => {
                    let has_ref = m.evidence_ref.map(str::trim).is_some_and(|s| !s.is_empty());
                    let has_source = m
                        .external_source
                        .map(str::trim)
                        .is_some_and(|s| !s.is_empty());
                    let has_proc = m.procedure.map(str::trim).is_some_and(|s| !s.is_empty());
                    if !has_ref && !has_source && !has_proc {
                        return Err(WorkflowBoardRepoError::InvalidInput(format!(
                            "external AC `{cid}` requires evidence_ref, external_source, or procedure"
                        )));
                    }
                }
                other => {
                    return Err(WorkflowBoardRepoError::InvalidInput(format!(
                        "unsupported AC evidence method `{other}`"
                    )));
                }
            }
        }
    }
    Ok(())
}
