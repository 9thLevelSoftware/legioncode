//! Agent session scanner & importer (`agentSessions.scan`, `agentSessions.import`)
//! reading local `~/.claude/projects` and `~/.codex` history safely.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use legion_contracts::ids::{CommandId, ProjectId, ThreadId};
use legion_contracts::orchestration::{
    CreationSource, ModelSelection, OrchestrationActor, OrchestrationCommand,
    ProviderInteractionMode, ThreadCreateCommand,
};
use legion_contracts::policy::RuntimeMode;
use legion_persistence::project_store::{
    CreateProjectInput, create_project, ensure_project_store_schema,
    find_project_by_id_or_workspace,
};
use legion_protocol::rpc_codec::RpcExit;
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    AppState, parse_optional_payload, parse_payload, rpc_fail_obj, utc_now_iso, with_db_mut,
};

const MAX_SCAN_FILES: usize = 500;

pub(crate) fn scan(_state: &AppState, payload: Value) -> RpcExit {
    let _: Value = match parse_optional_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let mut projects_map: BTreeMap<String, Value> = BTreeMap::new();
    let mut total_sessions = 0_usize;

    if let Some(home) = home_dir() {
        let claude_projects = home.join(".claude").join("projects");
        scan_claude_projects_dir(&claude_projects, &mut projects_map, &mut total_sessions);
    }

    let candidates: Vec<Value> = projects_map
        .into_iter()
        .map(|(root, entry)| {
            let count = entry
                .get("sessions")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            let title = Path::new(&root)
                .file_name()
                .and_then(|s| s.to_str())
                .filter(|s| !s.trim().is_empty())
                .unwrap_or(&root)
                .to_owned();
            json!({
                "path": root,
                "title": title,
                "sources": ["claudeAgent"],
                "threadCount": count,
                "lastActiveAt": Value::Null,
                "alreadyImported": false,
                "git": Value::Null,
            })
        })
        .collect();
    RpcExit::Success {
        value: json!({
            "candidates": candidates,
            "scannedAt": utc_now_iso(),
            "truncated": false,
        }),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ImportPayload {
    project_id: Option<String>,
    expected_workspace_root: Option<String>,
    projects: Vec<ImportProjectEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportProjectEntry {
    workspace_root: String,
    title: Option<String>,
    #[serde(default)]
    sessions: Vec<ImportSessionEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportSessionEntry {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    title: Option<String>,
}

pub(crate) async fn import(state: &AppState, payload: Value) -> RpcExit {
    let req: ImportPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if let Some(ref pid) = req.project_id {
        let lookup = with_db_mut(state, |conn| {
            ensure_project_store_schema(conn).map_err(|e| {
                rpc_fail_obj(json!({
                    "_tag": "AgentSessionScanError",
                    "operation": "read-projects",
                    "cause": e.to_string(),
                }))
            })?;
            Ok(find_project_by_id_or_workspace(&*conn, pid).ok().flatten())
        });
        match lookup {
            Ok(Some(project)) => {
                if let Some(ref expected) = req.expected_workspace_root
                    && project.workspace_root != *expected
                {
                    return rpc_fail_obj(json!({
                        "_tag": "AgentSessionImportProjectChangedError",
                        "projectId": pid,
                    }));
                }
                return RpcExit::Success {
                    value: json!({ "importedCount": 0, "skippedCount": 0 }),
                };
            }
            Ok(None) => {
                return rpc_fail_obj(json!({
                    "_tag": "AgentSessionImportProjectNotFoundError",
                    "projectId": pid,
                }));
            }
            Err(e) => return e,
        }
    }
    let mut imported_count = 0_usize;
    for proj in req.projects {
        match import_project_sessions(state, proj).await {
            Ok(count) => imported_count += count,
            Err(e) => return e,
        }
    }

    RpcExit::Success {
        value: json!({
            "importedCount": imported_count,
            "skippedCount": 0,
        }),
    }
}

async fn import_project_sessions(
    state: &AppState,
    proj: ImportProjectEntry,
) -> Result<usize, RpcExit> {
    let root = proj.workspace_root.trim();
    if root.is_empty() {
        return Ok(0);
    }
    let project_id_str = with_db_mut(state, |conn| {
        ensure_project_store_schema(conn).map_err(|e| {
            rpc_fail_obj(json!({
                "_tag": "AgentSessionScanError",
                "operation": "read-projects",
                "cause": e.to_string(),
            }))
        })?;
        let rec =
            if let Some(existing) = find_project_by_id_or_workspace(&*conn, root).ok().flatten() {
                existing
            } else {
                let mut input = CreateProjectInput::new(root);
                if let Some(ref t) = proj.title {
                    input = input.with_title(t);
                }
                create_project(&*conn, &input).map_err(|e| {
                    rpc_fail_obj(json!({
                        "_tag": "AgentSessionScanError",
                        "operation": "read-projects",
                        "cause": e.to_string(),
                    }))
                })?
            };
        Ok(rec.id)
    })?;

    let mut imported = 0_usize;
    for sess in proj.sessions {
        let tid_str = format!("thread-{}", Uuid::new_v4());
        let title = sess
            .title
            .or(sess.id)
            .unwrap_or_else(|| "Imported session".to_owned());
        if let (Ok(cmd_id), Ok(tid), Ok(pid), Ok(ms)) = (
            CommandId::new(format!("import-{}", Uuid::new_v4())),
            ThreadId::new(tid_str.clone()),
            ProjectId::new(&project_id_str),
            serde_json::from_value::<ModelSelection>(json!({
                "instanceId": "claudeAgent",
                "model": "claude-sonnet-4-5",
            })),
        ) {
            let cmd = OrchestrationCommand::ThreadCreate(ThreadCreateCommand {
                created_by: OrchestrationActor::User,
                creation_source: CreationSource::Web,
                command_id: cmd_id,
                thread_id: tid,
                project_id: pid,
                title,
                generate_title: Some(false),
                model_selection: ms,
                runtime_mode: RuntimeMode::ApprovalRequired,
                interaction_mode: ProviderInteractionMode::Default,
                branch: None,
                worktree_path: None,
                link_origin: None,
                delegated_from: None,
                imported_native_thread: None,
                initial_message: None,
            });
            if state.orchestrator.dispatch_now(&cmd).await.is_ok() {
                imported += 1;
            }
        }
    }
    Ok(imported)
}

fn scan_claude_projects_dir(
    dir: &Path,
    projects: &mut BTreeMap<String, Value>,
    total_sessions: &mut usize,
) {
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    let mut seen_files = 0_usize;
    for proj_entry in rd.flatten() {
        if !proj_entry.path().is_dir() {
            continue;
        }
        let Ok(sub) = fs::read_dir(proj_entry.path()) else {
            continue;
        };
        let mut sessions = Vec::new();
        let mut workspace_root: Option<String> = None;
        for file_entry in sub.flatten() {
            if seen_files >= MAX_SCAN_FILES {
                break;
            }
            let p = file_entry.path();
            if p.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            seen_files += 1;
            let stem = p
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("session")
                .to_owned();
            if workspace_root.is_none()
                && let Ok(content) = fs::read_to_string(&p)
                && let Some(first_line) = content.lines().next()
                && let Ok(parsed) = serde_json::from_str::<Value>(first_line)
            {
                workspace_root = parsed.get("cwd").and_then(Value::as_str).map(str::to_owned);
            }
            sessions.push(json!({
                "id": stem,
                "source": "claude",
                "title": stem,
                "transcriptPath": p.to_string_lossy(),
            }));
            *total_sessions += 1;
        }
        if let Some(root) = workspace_root
            && !sessions.is_empty()
        {
            projects.insert(
                root.clone(),
                json!({
                    "workspaceRoot": root,
                    "source": "claude",
                    "sessions": sessions,
                }),
            );
        }
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}
