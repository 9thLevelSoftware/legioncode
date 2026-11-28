#![forbid(unsafe_code)]
//! Config, policy, approval, execution, decision, and assumption helpers for `.legion-cli/` scan.

use std::fs;
use std::path::Path;

use legion_contracts::workflow_board::{
    WorkflowBoardCliImportHistoricalReceipt, WorkflowBoardCliImportHistoricalReceiptKind,
    WorkflowBoardCliImportHistoricalStatus, WorkflowBoardCliImportMissingLogNote,
    WorkflowBoardCliImportPolicyStatus, WorkflowBoardCliImportUnsupportedPolicy,
    WorkflowBoardRequestError, WorkflowBoardRequestErrorCode,
};
use serde_json::Value;

use crate::workflow_board_cli_yaml::{parse_markdown_frontmatter, parse_yaml_str};
use crate::workflow_board_fs::is_symlink_or_reparse_point;

#[derive(Debug, Clone)]
pub(crate) struct ScannedDecision {
    pub id: String,
    pub prompt: String,
    pub status: String,
    pub answer: Option<String>,
    pub rationale: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct ScannedExecutionState {
    pub spec_id: Option<String>,
    pub status: Option<String>,
    pub blocker: Option<String>,
    pub updated_at: Option<String>,
    pub missing_log_relpath: Option<String>,
}

pub(crate) fn yaml_err(path_label: &str, detail: &str) -> WorkflowBoardRequestError {
    WorkflowBoardRequestError::new(
        WorkflowBoardRequestErrorCode::InvalidInput,
        format!("Malformed YAML in `{path_label}`: {detail}"),
    )
}

pub(crate) fn read_yaml_file(
    path: &Path,
    rel_path: &str,
    digest_parts: &mut Vec<(String, Vec<u8>)>,
) -> Result<Option<Value>, WorkflowBoardRequestError> {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return Ok(None);
    };
    if is_symlink_or_reparse_point(&meta) || !meta.is_file() {
        return Ok(None);
    }
    let bytes = fs::read(path).map_err(|e| {
        WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::InvalidInput,
            format!("Failed to read `{rel_path}`: {e}"),
        )
    })?;
    let text = std::str::from_utf8(&bytes).map_err(|e| yaml_err(rel_path, &e.to_string()))?;
    let val = parse_yaml_str(text, rel_path).map_err(|e| yaml_err(rel_path, &e))?;
    digest_parts.push((rel_path.to_owned(), bytes));
    Ok(Some(val))
}

pub(crate) fn read_markdown_file(
    path: &Path,
    rel_path: &str,
    digest_parts: &mut Vec<(String, Vec<u8>)>,
) -> Result<Option<(Option<Value>, String)>, WorkflowBoardRequestError> {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return Ok(None);
    };
    if is_symlink_or_reparse_point(&meta) || !meta.is_file() {
        return Ok(None);
    }
    let bytes = fs::read(path).map_err(|e| {
        WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::InvalidInput,
            format!("Failed to read `{rel_path}`: {e}"),
        )
    })?;
    let text = std::str::from_utf8(&bytes).map_err(|e| yaml_err(rel_path, &e.to_string()))?;
    let parsed = parse_markdown_frontmatter(text, rel_path).map_err(|e| yaml_err(rel_path, &e))?;
    digest_parts.push((rel_path.to_owned(), bytes));
    Ok(Some(parsed))
}

pub(crate) type ScannedPoliciesOutput = (
    Vec<WorkflowBoardCliImportUnsupportedPolicy>,
    Option<String>,
    Option<String>,
);

pub(crate) type ScannedDecisionsOutput = (Vec<ScannedDecision>, Vec<String>, Vec<String>);

pub(crate) fn scan_policies(
    cli_root: &Path,
    cli_rel: &str,
    config_val: Option<&Value>,
    digest_parts: &mut Vec<(String, Vec<u8>)>,
) -> Result<ScannedPoliciesOutput, WorkflowBoardRequestError> {
    let mut out = Vec::new();
    let cfg_path = format!("{cli_rel}/config.yaml");
    if let Some(cfg) = config_val {
        collect_config_policies(cfg, &cfg_path, &mut out);
    }
    let (assurance_summary, assurance_json) =
        scan_assurance_sidecars(cli_root, cli_rel, digest_parts, &mut out)?;
    Ok((out, assurance_summary, assurance_json))
}

fn collect_config_policies(
    cfg: &Value,
    cfg_path: &str,
    out: &mut Vec<WorkflowBoardCliImportUnsupportedPolicy>,
) {
    if let Some(val) = cfg.get("sandbox").and_then(|s| s.get("requireHardened")) {
        let blocks = val.as_bool().unwrap_or(true);
        out.push(WorkflowBoardCliImportUnsupportedPolicy {
            key: "sandbox.requireHardened".to_owned(),
            category: "sandbox".to_owned(),
            summary: "CLI `sandbox.requireHardened` is not natively enforced; LegionCode uses runtimeMode permissions".to_owned(),
            configured_value: Some(val.to_string()),
            status: WorkflowBoardCliImportPolicyStatus::Unsupported,
            blocks_execution: blocks,
            source_path: cfg_path.to_owned(),
        });
    }
    if let Some(val) = cfg.get("qa").and_then(|q| q.get("passScore")) {
        out.push(WorkflowBoardCliImportUnsupportedPolicy {
            key: "qa.passScore".to_owned(),
            category: "qa".to_owned(),
            summary: "CLI `qa.passScore` numeric score threshold is not natively enforced; LegionCode uses review verdict & AC evidence".to_owned(),
            configured_value: Some(val.to_string()),
            status: WorkflowBoardCliImportPolicyStatus::Unsupported,
            blocks_execution: true,
            source_path: cfg_path.to_owned(),
        });
    }
    if let Some(val) = cfg.get("skills").and_then(|s| s.get("trustKeys")) {
        out.push(WorkflowBoardCliImportUnsupportedPolicy {
            key: "skills.trustKeys".to_owned(),
            category: "skills".to_owned(),
            summary: "CLI `skills.trustKeys` cryptographic trust policy is not natively enforced by LegionCode".to_owned(),
            configured_value: Some(val.to_string()),
            status: WorkflowBoardCliImportPolicyStatus::Unsupported,
            blocks_execution: true,
            source_path: cfg_path.to_owned(),
        });
    }
    if let Some(val) = cfg.get("mcpHttpToolAllowlist") {
        let non_empty = match val {
            Value::Null => false,
            Value::Array(a) => !a.is_empty(),
            Value::Object(m) => !m.is_empty(),
            Value::String(s) => !s.trim().is_empty(),
            Value::Bool(b) => *b,
            Value::Number(_) => true,
        };
        out.push(WorkflowBoardCliImportUnsupportedPolicy {
            key: "mcpHttpToolAllowlist".to_owned(),
            category: "mcp".to_owned(),
            summary: "CLI `mcpHttpToolAllowlist` is surfaced for awareness; LegionCode uses scoped MCP tools".to_owned(),
            configured_value: Some(val.to_string()),
            status: WorkflowBoardCliImportPolicyStatus::Unsupported,
            blocks_execution: non_empty,
            source_path: cfg_path.to_owned(),
        });
    }
    if let Some(val) = cfg.get("dashboard") {
        out.push(WorkflowBoardCliImportUnsupportedPolicy {
            key: "dashboard".to_owned(),
            category: "dashboard".to_owned(),
            summary: "CLI local `dashboard` server configuration is ignored by LegionCode"
                .to_owned(),
            configured_value: Some(val.to_string()),
            status: WorkflowBoardCliImportPolicyStatus::Unsupported,
            blocks_execution: false,
            source_path: cfg_path.to_owned(),
        });
    }
}

fn scan_assurance_sidecars(
    cli_root: &Path,
    cli_rel: &str,
    digest_parts: &mut Vec<(String, Vec<u8>)>,
    out: &mut Vec<WorkflowBoardCliImportUnsupportedPolicy>,
) -> Result<(Option<String>, Option<String>), WorkflowBoardRequestError> {
    let mut assurance_summary = None;
    let mut assurance_json = None;
    let wf_dir = cli_root.join("workflow");
    if let Ok(entries) = fs::read_dir(&wf_dir) {
        let mut assurance_files: Vec<_> = entries
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                let is_yaml = Path::new(&name).extension().is_some_and(|ext| {
                    ext.eq_ignore_ascii_case("yaml") || ext.eq_ignore_ascii_case("yml")
                });
                if name.starts_with("assurance") && is_yaml {
                    Some((name, e.path()))
                } else {
                    None
                }
            })
            .collect();
        assurance_files.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, path) in assurance_files {
            let rel = format!("{cli_rel}/workflow/{name}");
            if let Some(val) = read_yaml_file(&path, &rel, digest_parts)? {
                let serialized = serde_json::to_string(&val).ok();
                if assurance_json.is_none() {
                    assurance_json.clone_from(&serialized);
                    assurance_summary = Some(format!(
                        "Adopted assurance sidecar `{rel}` requires operator acknowledgement"
                    ));
                }
                out.push(WorkflowBoardCliImportUnsupportedPolicy {
                    key: "workflow.assurance".to_owned(),
                    category: "assurance".to_owned(),
                    summary: format!(
                        "CLI assurance sidecar `{name}` requires explicit operator acknowledgement"
                    ),
                    configured_value: serialized,
                    status: WorkflowBoardCliImportPolicyStatus::Unsupported,
                    blocks_execution: true,
                    source_path: rel,
                });
            }
        }
    }
    Ok((assurance_summary, assurance_json))
}

pub(crate) fn parse_execution_state(
    ws_root: &Path,
    cli_rel: &str,
    exec_val: Option<&Value>,
) -> (
    ScannedExecutionState,
    Option<WorkflowBoardCliImportMissingLogNote>,
    Option<WorkflowBoardCliImportHistoricalReceipt>,
) {
    let Some(val) = exec_val else {
        return (
            ScannedExecutionState {
                spec_id: None,
                status: None,
                blocker: None,
                updated_at: None,
                missing_log_relpath: None,
            },
            None,
            None,
        );
    };
    let str_field = |k: &str| {
        val.get(k)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let spec_id = str_field("specId");
    let status = str_field("status");
    let blocker = str_field("blocker");
    let updated_at = str_field("updatedAt");

    let (failed_command, missing_log_relpath) = blocker
        .as_deref()
        .map_or((None, None), extract_blocker_command_and_log);

    let missing_note = missing_log_relpath.as_deref().and_then(|rel| {
        let candidate = rel
            .split('/')
            .fold(ws_root.to_path_buf(), |p, seg| p.join(seg));
        if candidate.is_file() {
            None
        } else {
            Some(WorkflowBoardCliImportMissingLogNote {
                task_id: None,
                check_id: Some("chk-cli-exec-1".to_owned()),
                relative_path: rel.to_owned(),
                command: failed_command.clone(),
                note: format!(
                    "Gitignored CLI cache log `{rel}` is absent on disk; historical blocker preserved from execution.yaml"
                ),
            })
        }
    });

    let receipt = Some(WorkflowBoardCliImportHistoricalReceipt {
        id: "hist-exec-1".to_owned(),
        kind: WorkflowBoardCliImportHistoricalReceiptKind::Execution,
        status: WorkflowBoardCliImportHistoricalStatus::Historical,
        source_path: format!("{cli_rel}/workflow/execution.yaml"),
        recorded_at: updated_at.clone(),
        recorded_by: Some("legion-cli".to_owned()),
        summary: blocker.clone().unwrap_or_else(|| {
            format!(
                "Historical CLI execution status: {}",
                status.as_deref().unwrap_or("unknown")
            )
        }),
        requires_native_revalidation: true,
    });

    (
        ScannedExecutionState {
            spec_id,
            status,
            blocker,
            updated_at,
            missing_log_relpath,
        },
        missing_note,
        receipt,
    )
}

fn extract_blocker_command_and_log(blocker: &str) -> (Option<String>, Option<String>) {
    let mut log_path = None;
    let mut cmd_slice = blocker;
    if let Some(idx) = blocker.find("(log:") {
        cmd_slice = blocker[..idx].trim();
        let after = blocker[idx + 5..].trim();
        if let Some(end) = after.find(')') {
            let candidate = after[..end].trim();
            if !candidate.is_empty() {
                log_path = Some(candidate.replace('\\', "/"));
            }
        }
    }
    let command = if let Some((_, rhs)) = cmd_slice.split_once(':') {
        let trimmed = rhs.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_owned())
        }
    } else {
        None
    };
    (command, log_path)
}

pub(crate) fn scan_decisions_and_assumptions(
    cli_root: &Path,
    cli_rel: &str,
    digest_parts: &mut Vec<(String, Vec<u8>)>,
) -> Result<ScannedDecisionsOutput, WorkflowBoardRequestError> {
    let mut preserved = Vec::new();
    let mut decisions = Vec::new();
    let dec_dir = cli_root.join("decisions");
    if let Ok(entries) = fs::read_dir(&dec_dir) {
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
            let name = entry.file_name().to_string_lossy().into_owned();
            let rel = format!("{cli_rel}/decisions/{name}");
            if let Some((fm, body)) = read_markdown_file(&entry.path(), &rel, digest_parts)? {
                preserved.push(rel);
                let id = fm
                    .as_ref()
                    .and_then(|v| v.get("id"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map_or_else(|| name.trim_end_matches(".md").to_owned(), str::to_owned);
                let raw_status = fm
                    .as_ref()
                    .and_then(|v| v.get("status"))
                    .and_then(Value::as_str)
                    .unwrap_or("accepted")
                    .trim();
                let status = match raw_status {
                    "unresolved" | "resolved" | "accepted" | "rejected" | "superseded" => {
                        raw_status
                    }
                    "open" => "unresolved",
                    _ => "accepted",
                }
                .to_owned();
                let summary = fm
                    .as_ref()
                    .and_then(|v| v.get("summary"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map_or_else(
                        || body.lines().next().unwrap_or(&id).trim().to_owned(),
                        str::to_owned,
                    );
                decisions.push(ScannedDecision {
                    id,
                    prompt: summary.clone(),
                    status,
                    answer: Some(summary),
                    rationale: if body.is_empty() { None } else { Some(body) },
                });
            }
        }
    }

    let mut assumption_ids = Vec::new();
    let asm_dir = cli_root.join("assumptions");
    if let Ok(entries) = fs::read_dir(&asm_dir) {
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
            let name = entry.file_name().to_string_lossy().into_owned();
            let rel = format!("{cli_rel}/assumptions/{name}");
            if let Some((fm, _)) = read_markdown_file(&entry.path(), &rel, digest_parts)? {
                preserved.push(rel);
                let id = fm
                    .as_ref()
                    .and_then(|v| v.get("id"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map_or_else(|| name.trim_end_matches(".md").to_owned(), str::to_owned);
                assumption_ids.push(id);
            }
        }
    }

    Ok((decisions, assumption_ids, preserved))
}
