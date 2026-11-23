//! Target path resolution for signed `assets.createUrl` resources.

use std::fs;
use std::path::{Path, PathBuf};

use legion_contracts::ids::ThreadId;
use legion_orchestrator::projections::query_wire_projects;
use legion_protocol::rpc_codec::RpcExit;
use serde_json::{Value, json};

use super::{AppState, rpc_fail_obj};

const PROJECT_FAVICON_FALLBACK: &str = "project-favicon-missing";
const FAVICON_CANDIDATES: &[&str] = &[
    "favicon.svg",
    "favicon.png",
    "favicon.ico",
    "public/favicon.svg",
    "public/favicon.png",
    "public/favicon.ico",
    "app/favicon.ico",
    "app/icon.svg",
    "app/icon.png",
    "src/app/favicon.ico",
    "src/app/icon.svg",
    "src/app/icon.png",
];

fn pick_str<'a>(val: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|k| val.get(*k).and_then(Value::as_str))
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn sanitize_url_filename(name: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_whitespace() || matches!(c, '/' | '\\' | '?' | '#') {
                '_'
            } else {
                c
            }
        })
        .collect();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        "asset".to_owned()
    } else {
        cleaned
    }
}

fn default_workspace_root(state: &AppState) -> PathBuf {
    state
        .runtime_http
        .allowed_workspace_roots
        .first()
        .cloned()
        .unwrap_or_else(|| state.paths.state_dir.clone())
}

fn resolve_thread_root(state: &AppState, thread_id: Option<&str>) -> PathBuf {
    let projects = state
        .orchestrator
        .sink()
        .db()
        .lock()
        .ok()
        .and_then(|conn| query_wire_projects(&conn).ok())
        .map_or_default(|snap| snap.projects);
    if let Some(tid) = thread_id
        && let Ok(parsed) = ThreadId::new(tid)
        && let Ok(decider) = state.orchestrator.load_thread_state(&parsed)
        && let Some(thread) = decider.thread
    {
        if let Some(wt) = thread.worktree_path.filter(|s| !s.trim().is_empty()) {
            return PathBuf::from(wt);
        }
        if let Some(proj) = projects.iter().find(|p| p.id == thread.project_id.as_str()) {
            return PathBuf::from(&proj.workspace_root);
        }
    }
    default_workspace_root(state)
}

fn resolve_project_favicon(state: &AppState, resource: &Value) -> (PathBuf, Option<String>) {
    let cwd = pick_str(resource, &["cwd", "workspaceRoot"])
        .map_or_else(|| default_workspace_root(state), PathBuf::from);
    let hint = pick_str(resource, &["path"])
        .map(str::to_owned)
        .or_else(|| {
            state
                .orchestrator
                .sink()
                .db()
                .lock()
                .ok()
                .and_then(|conn| query_wire_projects(&conn).ok())
                .and_then(|snap| {
                    let cwd_str = cwd.to_string_lossy();
                    snap.projects
                        .into_iter()
                        .find(|p| p.workspace_root == cwd_str.as_ref())
                        .and_then(|p| p.favicon_path)
                })
        });
    if let Some(rel_or_abs) = hint {
        let candidate = Path::new(&rel_or_abs);
        let full = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            cwd.join(candidate)
        };
        if full.is_file() {
            return (full, None);
        }
    }
    for rel in FAVICON_CANDIDATES {
        let candidate = cwd.join(rel);
        if candidate.is_file() {
            return (candidate, None);
        }
    }
    (
        cwd.join(PROJECT_FAVICON_FALLBACK),
        Some(PROJECT_FAVICON_FALLBACK.to_owned()),
    )
}

fn resolve_attachment_target(state: &AppState, resource: &Value) -> (PathBuf, Option<String>) {
    let id = pick_str(resource, &["attachmentId"]).unwrap_or("attachment");
    let custom_name = pick_str(resource, &["fileName"]).map(sanitize_url_filename);
    if !id.contains('/')
        && !id.contains('\\')
        && !id.contains("..")
        && let Ok(entries) = fs::read_dir(&state.paths.attachments_dir)
    {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let s = name.to_string_lossy();
            if (s == id || s.starts_with(&format!("{id}."))) && entry.path().is_file() {
                return (entry.path(), custom_name);
            }
        }
    }
    (state.paths.attachments_dir.join(id), custom_name)
}

pub(crate) fn resolve_target_for_resource(
    state: &AppState,
    resource: &Value,
) -> Result<(PathBuf, Option<String>), RpcExit> {
    if !resource.is_object() {
        return Err(rpc_fail_obj(json!({
            "_tag": "AssetWorkspaceResolutionError",
            "resource": resource,
            "cause": "Invalid asset resource payload.",
        })));
    }
    let tag = pick_str(resource, &["_tag", "kind"]).unwrap_or("");
    let target = match tag {
        "project-favicon" => resolve_project_favicon(state, resource),
        "workspace-file" | "media-file" => {
            let root = pick_str(resource, &["workspaceRoot"]).map_or_else(
                || resolve_thread_root(state, pick_str(resource, &["threadId"])),
                PathBuf::from,
            );
            let rel = pick_str(resource, &["path", "baseRelativePath"]).unwrap_or("asset");
            let path = Path::new(rel);
            let resolved = if tag == "media-file" && path.is_absolute() {
                path.to_path_buf()
            } else {
                root.join(path)
            };
            let display = resolved
                .file_name()
                .and_then(|s| s.to_str())
                .map(sanitize_url_filename);
            (resolved, display)
        }
        "draft-workspace-file" => {
            let cwd = pick_str(resource, &["cwd", "workspaceRoot"])
                .map_or_else(|| default_workspace_root(state), PathBuf::from);
            let rel = pick_str(resource, &["path", "baseRelativePath"]).unwrap_or("asset");
            let path = Path::new(rel);
            let resolved = if path.is_absolute() {
                path.to_path_buf()
            } else {
                cwd.join(path)
            };
            let display = resolved
                .file_name()
                .and_then(|s| s.to_str())
                .map(sanitize_url_filename);
            (resolved, display)
        }
        "attachment" => resolve_attachment_target(state, resource),
        _ => (
            state.paths.state_dir.join("asset"),
            Some("asset".to_owned()),
        ),
    };
    Ok(target)
}
