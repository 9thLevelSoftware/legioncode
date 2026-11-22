//! Multi-repo workspace folder scanner (`Child` + `.code-workspace`, `60863edc41` `#35925`).

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use t3_contracts::workspace_folders::{
    PROJECT_TRACKED_FOLDERS_MAX, ProjectTrackedFolder, WorkspaceFolder, WorkspaceFolderState,
    WorkspaceFoldersError, WorkspaceFoldersOperation, WorkspaceScanFoldersInput,
    WorkspaceScanFoldersResult, WorkspaceScannedFolder, WorkspaceScannedFolderSource,
};

pub use crate::workspace_folders_parse::{
    is_within_root, lexical_clean, parse_gitmodules_paths, strip_lenient_jsonc,
};

const FOLDER_NAME_MAX: usize = 128;
const CODE_WORKSPACE_EXTENSION: &str = ".code-workspace";
const T3_PROJECT_FILE_NAME: &str = "t3.json";

fn bounded_name(name: &str) -> String {
    let mut truncated: String = name.chars().take(FOLDER_NAME_MAX).collect();
    truncated = truncated.trim().to_owned();
    truncated
}

fn owns_history(folder: &Path) -> bool {
    fs::metadata(folder.join(".git")).is_ok_and(|meta| meta.is_dir())
}

fn real_path_or_self(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| lexical_clean(path))
}

#[derive(Debug, Deserialize)]
struct RawCodeWorkspace {
    #[serde(default)]
    folders: Vec<RawCodeWorkspaceFolder>,
}

#[derive(Debug, Deserialize)]
struct RawCodeWorkspaceFolder {
    path: Option<String>,
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawT3Project {
    #[serde(default)]
    ignore_repositories: Vec<String>,
}

fn read_code_workspace_folders(root: &Path, file_name: &str) -> Option<Vec<(PathBuf, String)>> {
    let raw = fs::read_to_string(root.join(file_name)).ok()?;
    let cleaned = strip_lenient_jsonc(&raw);
    let parsed: RawCodeWorkspace = serde_json::from_str(&cleaned).ok()?;
    let mut out = Vec::new();
    for entry in parsed.folders {
        let Some(p) = entry
            .path
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        let resolved = lexical_clean(&root.join(p));
        let explicit = entry.name.as_deref().map_or_default(bounded_name);
        let fallback = resolved
            .file_name()
            .map_or_default(|n| bounded_name(&n.to_string_lossy()));
        out.push((
            resolved,
            if explicit.is_empty() {
                fallback
            } else {
                explicit
            },
        ));
    }
    Some(out)
}

/// Scans `input.workspace_root` for repositories (`Child` directories & `.code-workspace` entries),
/// excluding `.gitmodules` submodules and self/ancestor symlinks (`60863edc41` `#35925`).
pub fn scan_workspace_folders(
    input: &WorkspaceScanFoldersInput,
    limit: Option<usize>,
) -> Result<WorkspaceScanFoldersResult, WorkspaceFoldersError> {
    let raw_root = input.workspace_root.trim();
    if raw_root.is_empty() {
        return Err(WorkspaceFoldersError {
            operation: WorkspaceFoldersOperation::Scan,
            detail: "Workspace root must not be empty.".to_owned(),
        });
    }
    let root = lexical_clean(Path::new(raw_root));
    let mut names: Vec<String> = fs::read_dir(&root)
        .map_err(|_| WorkspaceFoldersError {
            operation: WorkspaceFoldersOperation::Scan,
            detail: format!("Could not read {}.", root.display()),
        })?
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();

    let code_ws_file = names
        .iter()
        .find(|n| n.ends_with(CODE_WORKSPACE_EXTENSION))
        .cloned();
    let listed = code_ws_file
        .as_deref()
        .and_then(|name| read_code_workspace_folders(&root, name));

    let raw_candidates = collect_scan_candidates(&root, &names, listed.as_deref());
    let submodules: HashSet<String> = fs::read_to_string(root.join(".gitmodules"))
        .ok()
        .map_or_default(|s| parse_gitmodules_paths(&s).into_iter().collect());
    let real_root = real_path_or_self(&root);
    let mut seen_real = HashSet::new();
    let mut folders = Vec::new();
    let cap = limit.unwrap_or(PROJECT_TRACKED_FOLDERS_MAX);

    for (cand_path, name, source) in raw_candidates {
        let resolved_real = real_path_or_self(&cand_path);
        if !seen_real.insert(resolved_real.clone()) || is_within_root(&real_root, &resolved_real) {
            continue;
        }
        let relative_path = cand_path
            .strip_prefix(&root)
            .ok()
            .map_or_default(|r| r.to_string_lossy().replace('\\', "/"));
        if submodules.contains(&relative_path) || !cand_path.exists() {
            continue;
        }
        folders.push(WorkspaceScannedFolder {
            path: cand_path.to_string_lossy().into_owned(),
            name,
            is_git_repository: cand_path.join(".git").exists(),
            source,
        });
        if folders.len() >= cap {
            break;
        }
    }

    Ok(WorkspaceScanFoldersResult {
        workspace_root: root.to_string_lossy().into_owned(),
        is_git_repository: root.join(".git").exists(),
        code_workspace_file: match (listed, code_ws_file) {
            (Some(_), Some(f)) => Some(root.join(f).to_string_lossy().into_owned()),
            _ => None,
        },
        folders,
    })
}

fn collect_scan_candidates(
    root: &Path,
    names: &[String],
    listed: Option<&[(PathBuf, String)]>,
) -> Vec<(PathBuf, String, WorkspaceScannedFolderSource)> {
    let mut candidates = Vec::new();
    if let Some(ws_entries) = listed {
        for (p, n) in ws_entries {
            candidates.push((
                p.clone(),
                n.clone(),
                WorkspaceScannedFolderSource::CodeWorkspace,
            ));
        }
    }
    for name in names
        .iter()
        .filter(|n| !n.starts_with('.') && n.as_str() != "node_modules")
    {
        let child = root.join(name);
        if owns_history(&child) {
            candidates.push((
                child,
                bounded_name(name),
                WorkspaceScannedFolderSource::Child,
            ));
        } else if !child.join(".git").exists()
            && child.is_dir()
            && let Ok(sub_rd) = fs::read_dir(&child)
        {
            let mut sub_names: Vec<String> = sub_rd
                .filter_map(Result::ok)
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|n| !n.starts_with('.') && n.as_str() != "node_modules")
                .collect();
            sub_names.sort();
            for sub in sub_names {
                let nested = child.join(&sub);
                if owns_history(&nested) {
                    candidates.push((
                        nested,
                        bounded_name(&sub),
                        WorkspaceScannedFolderSource::Child,
                    ));
                }
            }
        }
    }
    candidates
}

/// Discovers Git repositories inside `workspace_root`, filtering `t3.json` `ignoreRepositories` and
/// external symlink escapes (`60863edc41` `#35925`).
#[must_use]
pub fn discover_project_folders(workspace_root: &Path) -> Vec<ProjectTrackedFolder> {
    let input = WorkspaceScanFoldersInput {
        workspace_root: workspace_root.to_string_lossy().into_owned(),
    };
    let Ok(scan) = scan_workspace_folders(&input, Some(usize::MAX)) else {
        return Vec::new();
    };
    let root = PathBuf::from(&scan.workspace_root);
    let real_root = real_path_or_self(&root);
    let ignored: Vec<PathBuf> = fs::read_to_string(root.join(T3_PROJECT_FILE_NAME))
        .ok()
        .and_then(|raw| serde_json::from_str::<RawT3Project>(&strip_lenient_jsonc(&raw)).ok())
        .map_or_default(|proj| {
            proj.ignore_repositories
                .into_iter()
                .map(|entry| lexical_clean(&root.join(entry)))
                .collect()
        });

    scan.folders
        .into_iter()
        .filter(|folder| {
            if !folder.is_git_repository {
                return false;
            }
            let p = PathBuf::from(&folder.path);
            let is_ignored = ignored
                .iter()
                .any(|ign| p == *ign || is_within_root(&p, ign));
            if is_ignored || !owns_history(&p) {
                return false;
            }
            let real = real_path_or_self(&p);
            real != real_root && is_within_root(&real, &real_root)
        })
        .map(|folder| ProjectTrackedFolder {
            path: folder.path,
            name: folder.name,
            discovered: Some(true),
        })
        .collect()
}

/// Merges auto-discovered repos with user-added tracked folders up to [`PROJECT_TRACKED_FOLDERS_MAX`] (`60863edc41`).
#[must_use]
pub fn merge_project_folders(
    discovered: &[ProjectTrackedFolder],
    tracked_folders: &[ProjectTrackedFolder],
) -> Vec<ProjectTrackedFolder> {
    let found: HashSet<&str> = discovered.iter().map(|f| f.path.as_str()).collect();
    let added: Vec<ProjectTrackedFolder> = tracked_folders
        .iter()
        .filter(|f| f.discovered != Some(true) && !found.contains(f.path.as_str()))
        .take(PROJECT_TRACKED_FOLDERS_MAX)
        .cloned()
        .collect();
    let mut merged: Vec<ProjectTrackedFolder> = discovered
        .iter()
        .take(PROJECT_TRACKED_FOLDERS_MAX.saturating_sub(added.len()))
        .cloned()
        .collect();
    merged.extend(added);
    merged
}

/// Classifies [`ProjectTrackedFolder`] entries into [`WorkspaceFolder`] with `Git` / `Plain` / `Missing` state.
#[must_use]
pub fn classify_tracked_folders(folders: &[ProjectTrackedFolder]) -> Vec<WorkspaceFolder> {
    folders
        .iter()
        .map(|folder| {
            let path = Path::new(&folder.path);
            let state = if path.join(".git").exists() {
                WorkspaceFolderState::Git
            } else if path.exists() {
                WorkspaceFolderState::Plain
            } else {
                WorkspaceFolderState::Missing
            };
            WorkspaceFolder {
                name: folder.name.clone(),
                path: folder.path.clone(),
                state,
                discovered: folder.discovered,
            }
        })
        .collect()
}
