//! M2 Plan structural validators for lenses, alternatives, task DAGs, and AC mappings.

use std::collections::{HashMap, HashSet, VecDeque};

use super::m2_models::{
    WorkflowBoardAcEvidenceMapping, WorkflowBoardEvidenceMethod, WorkflowBoardPlanAlternative,
    WorkflowBoardPlanLens, WorkflowBoardPlanLensEntry, WorkflowBoardTaskContract,
};
use super::m2_validators::is_valid_non_git_artifact_key;

/// Validates design lens uniqueness and applicability summary/reason invariants.
pub fn validate_lenses_partial(lenses: &[WorkflowBoardPlanLensEntry]) -> Result<(), String> {
    let mut seen: HashSet<WorkflowBoardPlanLens> = HashSet::new();
    for entry in lenses {
        if !seen.insert(entry.lens) {
            return Err("duplicate design lens entry".to_owned());
        }
        match entry.applicable {
            Some(true)
                if entry
                    .summary
                    .as_deref()
                    .map(str::trim)
                    .is_none_or(str::is_empty) =>
            {
                return Err("applicable design lens requires non-empty summary".to_owned());
            }
            Some(false)
                if entry
                    .reason
                    .as_deref()
                    .map(str::trim)
                    .is_none_or(str::is_empty) =>
            {
                return Err("non-applicable design lens requires non-empty reason".to_owned());
            }
            Some(_) | None => {}
        }
    }
    Ok(())
}

/// Validates that exactly two materially distinct alternatives with unique IDs are supplied.
pub fn validate_alternatives(alternatives: &[WorkflowBoardPlanAlternative]) -> Result<(), String> {
    if alternatives.len() != 2 {
        return Err("expected exactly 2 design alternatives".to_owned());
    }
    let first = &alternatives[0];
    let second = &alternatives[1];
    if first.id.trim() == second.id.trim() {
        return Err("design alternative IDs must be distinct".to_owned());
    }
    if first.title.trim() == second.title.trim()
        && first.summary.trim() == second.summary.trim()
        && first.tradeoffs.trim() == second.tradeoffs.trim()
    {
        return Err("design alternatives must be materially distinct".to_owned());
    }
    Ok(())
}

/// Validates a single task contract's file ownership and non-self-dependency invariants.
pub fn validate_single_task_contract(task: &WorkflowBoardTaskContract) -> Result<(), String> {
    let tid = task.task_id.trim();
    if tid.is_empty() {
        return Err("taskId cannot be empty".to_owned());
    }
    if task.files_allowed.is_empty() {
        return Err(format!(
            "task `{tid}` must declare at least one filesAllowed path"
        ));
    }
    let forbidden: HashSet<&str> = task
        .files_forbidden
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .map(|s| s.trim())
        .collect();
    for allowed in &task.files_allowed {
        let trimmed = allowed.trim();
        if !is_valid_non_git_artifact_key(trimmed) {
            return Err(format!(
                "task `{tid}` has invalid filesAllowed path `{trimmed}`"
            ));
        }
        if forbidden.contains(trimmed) {
            return Err(format!(
                "task `{tid}` has `{trimmed}` in both filesAllowed and filesForbidden"
            ));
        }
    }
    for forb in task.files_forbidden.as_deref().unwrap_or(&[]) {
        let trimmed = forb.trim();
        if !is_valid_non_git_artifact_key(trimmed) {
            return Err(format!(
                "task `{tid}` has invalid filesForbidden path `{trimmed}`"
            ));
        }
    }
    for dep in task
        .depends_on
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .chain(task.blocked_by.as_deref().unwrap_or(&[]).iter())
        .chain(task.blocks.as_deref().unwrap_or(&[]).iter())
    {
        if dep.trim() == tid {
            return Err(format!("task `{tid}` cannot depend on or block itself"));
        }
    }
    Ok(())
}

/// Validates task contract uniqueness, disjoint file ownership, and acyclic task DAG invariants.
pub fn validate_tasks_dag_and_contracts(tasks: &[WorkflowBoardTaskContract]) -> Result<(), String> {
    let mut ids: HashSet<&str> = HashSet::new();
    let mut claimed_allowed: HashSet<&str> = HashSet::new();

    for task in tasks {
        validate_single_task_contract(task)?;
        let tid = task.task_id.trim();
        if !ids.insert(tid) {
            return Err(format!("duplicate or empty taskId `{tid}`"));
        }
        for allowed in &task.files_allowed {
            let norm = allowed.trim();
            if !claimed_allowed.insert(norm) {
                return Err(format!(
                    "file `{norm}` appears in filesAllowed of multiple tasks"
                ));
            }
        }
    }

    let mut dependents: HashMap<&str, Vec<&str>> = ids.iter().map(|&id| (id, Vec::new())).collect();
    let mut in_degree: HashMap<&str, usize> = ids.iter().map(|&id| (id, 0)).collect();
    let mut edges: HashSet<(&str, &str)> = HashSet::new();

    for task in tasks {
        let tid = task.task_id.trim();
        for d in task
            .depends_on
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .chain(task.blocked_by.as_deref().unwrap_or(&[]).iter())
        {
            let dep = d.trim();
            if !ids.contains(dep) || dep == tid {
                return Err(format!("task `{tid}` has invalid dependency `{dep}`"));
            }
            if edges.insert((dep, tid)) {
                if let Some(list) = dependents.get_mut(dep) {
                    list.push(tid);
                }
                if let Some(deg) = in_degree.get_mut(tid) {
                    *deg += 1;
                }
            }
        }
        for b in task.blocks.as_deref().unwrap_or(&[]) {
            let blocked = b.trim();
            if !ids.contains(blocked) || blocked == tid {
                return Err(format!("task `{tid}` has invalid block target `{blocked}`"));
            }
            if edges.insert((tid, blocked)) {
                if let Some(list) = dependents.get_mut(tid) {
                    list.push(blocked);
                }
                if let Some(deg) = in_degree.get_mut(blocked) {
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
        return Err("task dependency graph contains a cycle".to_owned());
    }
    Ok(())
}

/// Validates acceptance-criterion evidence mapping uniqueness and method fields.
pub fn validate_ac_mappings_partial(
    mappings: &[WorkflowBoardAcEvidenceMapping],
) -> Result<(), String> {
    let mut seen: HashSet<&str> = HashSet::new();
    for mapping in mappings {
        let cid = mapping.criterion_id.trim();
        if !seen.insert(cid) {
            return Err(format!("duplicate criterionId `{cid}` in acMappings"));
        }
        match mapping.method {
            WorkflowBoardEvidenceMethod::Automated => {
                let has_cmd = mapping.command.is_some();
                let has_checks = mapping
                    .check_ids
                    .as_ref()
                    .is_some_and(|ids| !ids.is_empty());
                if !has_cmd && !has_checks {
                    return Err(format!(
                        "automated AC mapping `{cid}` requires command or checkIds"
                    ));
                }
            }
            WorkflowBoardEvidenceMethod::Manual => {
                if mapping
                    .procedure
                    .as_deref()
                    .map(str::trim)
                    .is_none_or(str::is_empty)
                {
                    return Err(format!("manual AC mapping `{cid}` requires procedure"));
                }
            }
            WorkflowBoardEvidenceMethod::External => {
                let has_ref = mapping
                    .external_ref
                    .as_deref()
                    .map(str::trim)
                    .is_some_and(|s| !s.is_empty());
                let has_src = mapping
                    .external_source
                    .as_deref()
                    .map(str::trim)
                    .is_some_and(|s| !s.is_empty());
                let has_proc = mapping
                    .procedure
                    .as_deref()
                    .map(str::trim)
                    .is_some_and(|s| !s.is_empty());
                if !has_ref && !has_src && !has_proc {
                    return Err(format!(
                        "external AC mapping `{cid}` requires externalRef, externalSource, or procedure"
                    ));
                }
            }
        }
    }
    Ok(())
}
