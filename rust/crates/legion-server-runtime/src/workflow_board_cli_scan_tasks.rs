#![forbid(unsafe_code)]
//! Task contract, spec extraction, and historical approval parsing for `.legion-cli/` imports.

use std::fs;
use std::path::Path;

use legion_contracts::workflow_board::{
    WorkflowBoardAcceptanceCriterion, WorkflowBoardCliImportHistoricalReceipt,
    WorkflowBoardCliImportHistoricalReceiptKind, WorkflowBoardCliImportHistoricalStatus,
    WorkflowBoardCliImportMissingLogNote, WorkflowBoardRequestError, WorkflowBoardTaskStatus,
    WorkflowBoardTaskTestingMethod, is_valid_argv_command, is_valid_non_git_artifact_key,
};
use serde_json::Value;

use crate::workflow_board_cli_scan::{ScannedApproval, ScannedTask};
use crate::workflow_board_cli_scan_meta::{ScannedExecutionState, read_markdown_file, yaml_err};
use crate::workflow_board_fs::is_git_segment;

pub(crate) fn str_field(val: Option<&Value>, key: &str) -> Option<String> {
    val.and_then(|v| v.get(key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

pub(crate) fn discover_first_spec_dir(cli_root: &Path) -> Option<String> {
    let mut dirs: Vec<String> = fs::read_dir(cli_root.join("specs"))
        .ok()?
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    dirs.sort();
    dirs.into_iter().next()
}

pub(crate) fn extract_spec_content(
    spec_id: &str,
    fm: Option<&Value>,
    body: Option<&str>,
) -> (
    String,
    String,
    Vec<WorkflowBoardAcceptanceCriterion>,
    Vec<String>,
) {
    let title = str_field(fm, "title").unwrap_or_else(|| spec_id.to_owned());
    let brief = str_field(fm, "problem")
        .or_else(|| {
            body.map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| format!("Imported CLI specification `{spec_id}`"));
    let acs = fm
        .and_then(|v| v.get("acceptance"))
        .and_then(Value::as_array)
        .map_or_default(|arr| {
            arr.iter()
                .filter_map(|item| {
                    let id = item.get("id").and_then(Value::as_str)?.trim();
                    let text = item.get("statement").and_then(Value::as_str)?.trim();
                    if id.is_empty() || text.is_empty() {
                        None
                    } else {
                        Some(WorkflowBoardAcceptanceCriterion {
                            id: id.to_owned(),
                            text: text.to_owned(),
                        })
                    }
                })
                .collect::<Vec<_>>()
        });
    let mut constraints: Vec<String> = fm
        .and_then(|v| v.get("mustNotChange"))
        .and_then(Value::as_array)
        .map_or_default(|arr| {
            arr.iter()
                .filter_map(|v| {
                    v.as_str()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                })
                .collect()
        });
    if constraints.is_empty() {
        constraints.push("Preserve CLI import provenance and do not auto-commit".to_owned());
    }
    (title, brief, acs, constraints)
}

pub(crate) fn scan_tasks(
    cli_root: &Path,
    cli_rel: &str,
    exec_state: &ScannedExecutionState,
    exec_missing_note: Option<&WorkflowBoardCliImportMissingLogNote>,
    digest_parts: &mut Vec<(String, Vec<u8>)>,
) -> Result<(Vec<ScannedTask>, Vec<String>), WorkflowBoardRequestError> {
    let mut tasks = Vec::new();
    let mut paths = Vec::new();
    let tasks_dir = cli_root.join("tasks");
    let Ok(entries) = fs::read_dir(&tasks_dir) else {
        return Ok((tasks, paths));
    };
    let mut files: Vec<_> = entries
        .flatten()
        .filter(|e| {
            e.path()
                .extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("md"))
        })
        .collect();
    files.sort_by_key(std::fs::DirEntry::file_name);

    for entry in files {
        let (task, rel) = parse_single_task(
            &entry.path(),
            cli_rel,
            exec_state,
            exec_missing_note,
            digest_parts,
        )?;
        paths.push(rel);
        tasks.push(task);
    }
    Ok((tasks, paths))
}

fn parse_single_task(
    path: &Path,
    cli_rel: &str,
    exec_state: &ScannedExecutionState,
    exec_missing_note: Option<&WorkflowBoardCliImportMissingLogNote>,
    digest_parts: &mut Vec<(String, Vec<u8>)>,
) -> Result<(ScannedTask, String), WorkflowBoardRequestError> {
    let name = path
        .file_name()
        .map_or_default(|n| n.to_string_lossy().into_owned());
    let rel = format!("{cli_rel}/tasks/{name}");
    let Some((Some(fm), body)) = read_markdown_file(path, &rel, digest_parts)? else {
        return Err(yaml_err(
            &rel,
            "task markdown must contain YAML frontmatter",
        ));
    };
    let id = str_field(Some(&fm), "id").unwrap_or_else(|| name.trim_end_matches(".md").to_owned());
    let title = str_field(Some(&fm), "title").unwrap_or_else(|| id.clone());
    let status_str = str_field(Some(&fm), "status").unwrap_or_else(|| "todo".to_owned());
    let status = match status_str.as_str() {
        "pending" => WorkflowBoardTaskStatus::Pending,
        "in_progress" => WorkflowBoardTaskStatus::InProgress,
        "blocked" => WorkflowBoardTaskStatus::Blocked,
        "completed" => WorkflowBoardTaskStatus::Completed,
        "failed" => WorkflowBoardTaskStatus::Failed,
        "skipped" => WorkflowBoardTaskStatus::Skipped,
        _ => WorkflowBoardTaskStatus::Todo,
    };
    let priority = str_field(Some(&fm), "priority").unwrap_or_else(|| "P1".to_owned());
    let parent_id = str_field(Some(&fm), "parentId");
    let blocked_by = str_vec(fm.get("blockedBy"));
    let blocks = str_vec(fm.get("blocks"));
    let contract = fm.get("contract");
    let mut files_allowed: Vec<String> = str_vec(contract.and_then(|c| c.get("filesAllowed")))
        .into_iter()
        .filter(|p| is_valid_non_git_artifact_key(p))
        .collect();
    if files_allowed.is_empty() {
        files_allowed.push("README.md".to_owned());
    }
    let files_forbidden: Vec<String> = str_vec(contract.and_then(|c| c.get("filesForbidden")))
        .into_iter()
        .filter(|p| !p.split('/').any(is_git_segment))
        .collect();
    let expected_artifacts = str_vec(contract.and_then(|c| c.get("expectedArtifacts")));
    let verification_commands: Vec<Vec<String>> =
        str_vec(contract.and_then(|c| c.get("verificationCommands")))
            .into_iter()
            .filter_map(|cmd| split_quoted_command_tokens(&cmd))
            .filter(|argv| is_valid_argv_command(argv))
            .collect();
    let max_files_touched = contract
        .and_then(|c| c.get("maxFilesTouched"))
        .and_then(Value::as_i64);
    let notes = str_field(Some(&fm), "notes");
    let acceptance_ids = extract_acceptance_ids(notes.as_deref(), &body);

    let mut task_missing_notes = Vec::new();
    let mut blocker_summary = None;
    if status == WorkflowBoardTaskStatus::Blocked {
        blocker_summary = exec_state
            .blocker
            .clone()
            .or_else(|| Some(format!("Task `{id}` is blocked")));
        if let Some(note) = exec_missing_note {
            let mut cloned = note.clone();
            cloned.task_id = Some(id.clone());
            task_missing_notes.push(cloned);
        }
    }

    Ok((
        ScannedTask {
            id,
            title,
            status,
            priority,
            parent_id,
            blocked_by,
            blocks,
            files_allowed,
            files_forbidden,
            expected_artifacts,
            verification_commands,
            testing_method: WorkflowBoardTaskTestingMethod::TestFirst,
            acceptance_ids,
            blocker_summary,
            notes,
            max_files_touched,
            missing_log_notes: task_missing_notes,
        },
        rel,
    ))
}

fn split_quoted_command_tokens(raw: &str) -> Option<Vec<String>> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_token = false;
    let mut in_single = false;
    let mut in_double = false;
    let mut escape = false;

    for ch in raw.chars() {
        if escape {
            current.push(ch);
            in_token = true;
            escape = false;
            continue;
        }
        if in_single {
            if ch == '\'' {
                in_single = false;
            } else {
                current.push(ch);
            }
            in_token = true;
            continue;
        }
        if in_double {
            if ch == '\\' {
                escape = true;
            } else if ch == '"' {
                in_double = false;
            } else {
                current.push(ch);
            }
            in_token = true;
            continue;
        }
        match ch {
            '\\' => {
                escape = true;
                in_token = true;
            }
            '\'' => {
                in_single = true;
                in_token = true;
            }
            '"' => {
                in_double = true;
                in_token = true;
            }
            c if c.is_whitespace() => {
                if in_token {
                    tokens.push(std::mem::take(&mut current));
                    in_token = false;
                }
            }
            c => {
                current.push(c);
                in_token = true;
            }
        }
    }
    if in_single || in_double || escape {
        return None;
    }
    if in_token {
        tokens.push(current);
    }
    (!tokens.is_empty()).then_some(tokens)
}

fn str_vec(val: Option<&Value>) -> Vec<String> {
    val.and_then(Value::as_array).map_or_default(|arr| {
        arr.iter()
            .filter_map(|v| {
                v.as_str()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
            })
            .collect()
    })
}

fn extract_acceptance_ids(notes: Option<&str>, body: &str) -> Vec<String> {
    let mut ids = Vec::new();
    for source in [notes.unwrap_or(""), body] {
        for tok in source.split(|c: char| !c.is_ascii_alphanumeric() && c != '-') {
            if tok.starts_with("AC-") && !ids.iter().any(|existing| existing == tok) {
                ids.push(tok.to_owned());
            }
        }
    }
    ids
}

pub(crate) fn build_historical_approvals(
    spec_rel: &str,
    spec_val: Option<&Value>,
    plan_rel: &str,
    plan_val: Option<&Value>,
) -> (
    Vec<ScannedApproval>,
    Vec<WorkflowBoardCliImportHistoricalReceipt>,
) {
    let mut approvals = Vec::new();
    let mut receipts = Vec::new();
    if let Some(v) = spec_val {
        let dg =
            str_field(Some(v), "specFingerprint").unwrap_or_else(|| "sha256:cli-spec".to_owned());
        let at =
            str_field(Some(v), "approvedAt").unwrap_or_else(|| "2026-10-22T00:00:00Z".to_owned());
        let by = str_field(Some(v), "approvedBy").unwrap_or_else(|| "user".to_owned());
        approvals.push(ScannedApproval {
            id: "apr-spec".to_owned(),
            gate: "spec".to_owned(),
            artifact_digest: dg.clone(),
            spec_digest: Some(dg),
            plan_digest: None,
            tasks_digest: None,
            approved_by: format!("legion-cli:{by}"),
            approved_at: at.clone(),
            metadata_json: v.to_string(),
        });
        receipts.push(WorkflowBoardCliImportHistoricalReceipt {
            id: "hist-apr-spec".to_owned(),
            kind: WorkflowBoardCliImportHistoricalReceiptKind::SpecApproval,
            status: WorkflowBoardCliImportHistoricalStatus::Historical,
            source_path: spec_rel.to_owned(),
            recorded_at: Some(at),
            recorded_by: Some(by),
            summary: "Historical CLI spec approval (requires native revalidation before Ship)"
                .to_owned(),
            requires_native_revalidation: true,
        });
    }
    if let Some(v) = plan_val {
        let pdg =
            str_field(Some(v), "planFingerprint").unwrap_or_else(|| "sha256:cli-plan".to_owned());
        let sdg = str_field(Some(v), "specFingerprint");
        let tdg = str_field(Some(v), "taskFingerprint");
        let at =
            str_field(Some(v), "approvedAt").unwrap_or_else(|| "2026-10-22T00:00:00Z".to_owned());
        let by = str_field(Some(v), "approvedBy").unwrap_or_else(|| "user".to_owned());
        approvals.push(ScannedApproval {
            id: "apr-plan".to_owned(),
            gate: "plan".to_owned(),
            artifact_digest: pdg.clone(),
            spec_digest: sdg,
            plan_digest: Some(pdg),
            tasks_digest: tdg,
            approved_by: format!("legion-cli:{by}"),
            approved_at: at.clone(),
            metadata_json: v.to_string(),
        });
        receipts.push(WorkflowBoardCliImportHistoricalReceipt {
            id: "hist-apr-plan".to_owned(),
            kind: WorkflowBoardCliImportHistoricalReceiptKind::PlanApproval,
            status: WorkflowBoardCliImportHistoricalStatus::Historical,
            source_path: plan_rel.to_owned(),
            recorded_at: Some(at),
            recorded_by: Some(by),
            summary: "Historical CLI plan approval (requires native revalidation before Ship)"
                .to_owned(),
            requires_native_revalidation: true,
        });
    }
    (approvals, receipts)
}
