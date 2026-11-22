//! Multi-repo workspace folder scan/list and task-folder HTTP/runtime handlers (`#35925`).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use legion_contracts::permissions::EnvironmentScope;
use legion_contracts::workspace_folders::{
    ProjectTrackedFolder, TaskFolder, TaskFolderError, TaskFolderOperation, TaskFolderRepository,
    WorkspaceAddTaskFolderRepositoryInput, WorkspaceFolder, WorkspaceFolderState,
    WorkspaceListFoldersInput, WorkspaceListFoldersResult, WorkspaceScanFoldersInput,
    WorkspaceScanFoldersResult, WorkspaceTaskFolderInput, WorkspaceTaskFolderResult,
};
use legion_persistence::project_store::{
    ensure_project_store_schema, find_project_by_id_or_workspace,
};
use legion_scm::workspace_folders::{TaskFolderCoordinator, scan_workspace_folders};
use serde::Deserialize;

use crate::error::ServerRuntimeError;
use crate::http_api::RuntimeHttpState;
use crate::http_auth_guard::{authorize_request, parse_authorized_json};

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectIdQuery {
    pub project_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorktreePathQuery {
    pub worktree_path: Option<String>,
}

fn map_task_folder_err(err: TaskFolderError) -> ServerRuntimeError {
    match err.operation {
        TaskFolderOperation::Read
        | TaskFolderOperation::Plan
        | TaskFolderOperation::Create
        | TaskFolderOperation::Remove => ServerRuntimeError::Persistence(err.detail),
        TaskFolderOperation::AddRepository => ServerRuntimeError::BadRequest(err.detail),
    }
}

fn classify_folder_state(raw_path: &str) -> WorkspaceFolderState {
    let path = Path::new(raw_path);
    if !path.is_dir() {
        WorkspaceFolderState::Missing
    } else if path.join(".git").exists() {
        WorkspaceFolderState::Git
    } else {
        WorkspaceFolderState::Plain
    }
}

/// Resolves merged workspace folders for a project ID (`workspace.listFolders`, `#35925`).
pub fn list_workspace_folders_service(
    state: &RuntimeHttpState,
    input: &WorkspaceListFoldersInput,
) -> Result<WorkspaceListFoldersResult, ServerRuntimeError> {
    let conn = state
        .db()
        .lock()
        .map_err(|_| ServerRuntimeError::Persistence("db mutex poisoned".to_owned()))?;
    ensure_project_store_schema(&conn)?;
    let Some(project) = find_project_by_id_or_workspace(&*conn, input.project_id.as_str())? else {
        return Err(ServerRuntimeError::NotFound(format!(
            "Project `{}` was not found.",
            input.project_id
        )));
    };
    let saved_tracked: Vec<ProjectTrackedFolder> =
        read_saved_tracked_folders(state, input.project_id.as_str());
    let resolved = TaskFolderCoordinator::resolve_project_folders(
        Path::new(&project.workspace_root),
        &saved_tracked,
    );
    let folders = resolved
        .into_iter()
        .map(|entry| {
            let state = classify_folder_state(&entry.path);
            WorkspaceFolder {
                name: entry.name,
                path: entry.path,
                state,
                discovered: entry.discovered,
            }
        })
        .collect();
    Ok(WorkspaceListFoldersResult { folders })
}

/// Scans a directory root for candidate multi-repo folders (`workspace.scanFolders`, `#35925`).
pub fn scan_workspace_folders_service(
    input: &WorkspaceScanFoldersInput,
) -> Result<WorkspaceScanFoldersResult, ServerRuntimeError> {
    if input.workspace_root.trim().is_empty() {
        return Err(ServerRuntimeError::BadRequest(
            "workspaceRoot must not be empty".to_owned(),
        ));
    }
    scan_workspace_folders(input, None).map_err(|err| ServerRuntimeError::BadRequest(err.detail))
}

/// Looks up the active multi-repo task folder covering `worktree_path` (`#35925`).
pub fn get_workspace_task_folder_service(
    state: &RuntimeHttpState,
    input: &WorkspaceTaskFolderInput,
) -> Result<WorkspaceTaskFolderResult, ServerRuntimeError> {
    let trimmed = input.worktree_path.trim();
    if trimmed.is_empty() {
        return Err(ServerRuntimeError::BadRequest(
            "worktreePath must not be empty".to_owned(),
        ));
    }
    let conn = state
        .db()
        .lock()
        .map_err(|_| ServerRuntimeError::Persistence("db mutex poisoned".to_owned()))?;
    let task_folder =
        TaskFolderCoordinator::find(&conn, Path::new(trimmed)).map_err(map_task_folder_err)?;
    Ok(WorkspaceTaskFolderResult { task_folder })
}

/// Adds another repository checkout to a thread's active task folder (`#35925`).
pub fn add_task_folder_repository_service(
    state: &RuntimeHttpState,
    input: &WorkspaceAddTaskFolderRepositoryInput,
) -> Result<TaskFolder, ServerRuntimeError> {
    let repo_trimmed = input.repository_path.trim();
    if repo_trimmed.is_empty() {
        return Err(ServerRuntimeError::BadRequest(
            "repositoryPath must not be empty".to_owned(),
        ));
    }
    let conn = state
        .db()
        .lock()
        .map_err(|_| ServerRuntimeError::Persistence("db mutex poisoned".to_owned()))?;

    let worktree_path: Option<String> = conn
        .query_row(
            "SELECT json_extract(payload_json, '$.worktreePath')
             FROM orchestration_v2_projection_threads
             WHERE thread_id = ?1 AND deleted_at IS NULL",
            [input.thread_id.as_str()],
            |row| row.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());

    let Some(worktree) = worktree_path else {
        return Err(ServerRuntimeError::NotFound(format!(
            "Thread `{}` has no active worktree task folder.",
            input.thread_id
        )));
    };
    let Some(task_folder) =
        TaskFolderCoordinator::find(&conn, Path::new(&worktree)).map_err(map_task_folder_err)?
    else {
        return Err(ServerRuntimeError::NotFound(format!(
            "No multi-repo task folder tracks worktree `{worktree}`."
        )));
    };

    let repo_path = PathBuf::from(repo_trimmed);
    let anchor = Path::new(&task_folder.anchor);
    let worktree_in_task = anchor.join(repo_path.file_name().unwrap_or_default());
    let name = repo_path
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("repository")
        .to_owned();

    TaskFolderCoordinator::add_repository(
        &conn,
        &task_folder.root,
        TaskFolderRepository {
            repository_path: repo_path.to_string_lossy().into_owned(),
            worktree_path: worktree_in_task.to_string_lossy().into_owned(),
            name,
        },
        None,
    )
    .map_err(map_task_folder_err)
}

fn read_saved_tracked_folders(
    state: &RuntimeHttpState,
    project_id: &str,
) -> Vec<ProjectTrackedFolder> {
    state
        .project_settings_overrides
        .lock()
        .ok()
        .and_then(|guard| {
            guard
                .get(project_id)
                .and_then(|s| s.tracked_folders.clone())
        })
        .unwrap_or_default()
}

/// `GET /api/workspace/folders?projectId=...` (`OrchestrationRead`).
pub(crate) async fn handle_list_workspace_folders_get(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    Query(query): Query<ProjectIdQuery>,
) -> Result<Response, ServerRuntimeError> {
    let _principal = authorize_request(
        &state,
        &headers,
        &[EnvironmentScope::OrchestrationRead],
        false,
    )?;
    let Some(project_id) = query.project_id.filter(|s| !s.trim().is_empty()) else {
        return Err(ServerRuntimeError::BadRequest(
            "projectId query parameter is required".to_owned(),
        ));
    };
    let pid = legion_contracts::ids::ProjectId::new(project_id)
        .map_err(|err| ServerRuntimeError::BadRequest(err.to_string()))?;
    let res =
        list_workspace_folders_service(&state, &WorkspaceListFoldersInput { project_id: pid })?;
    Ok((StatusCode::OK, Json(res)).into_response())
}

/// `POST /api/workspace/folders` (`OrchestrationRead`).
pub(crate) async fn handle_list_workspace_folders_post(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ServerRuntimeError> {
    let (_principal, input): (_, WorkspaceListFoldersInput) = parse_authorized_json(
        &state,
        &headers,
        &[EnvironmentScope::OrchestrationRead],
        false,
        &body,
    )?;
    let res = list_workspace_folders_service(&state, &input)?;
    Ok((StatusCode::OK, Json(res)).into_response())
}

/// `POST /api/workspace/folders/scan` (`FilesystemRead`).
pub(crate) async fn handle_scan_workspace_folders(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ServerRuntimeError> {
    let (_principal, input): (_, WorkspaceScanFoldersInput) = parse_authorized_json(
        &state,
        &headers,
        &[EnvironmentScope::FilesystemRead],
        false,
        &body,
    )?;
    let res = scan_workspace_folders_service(&input)?;
    Ok((StatusCode::OK, Json(res)).into_response())
}

/// `GET /api/workspace/task-folders?worktreePath=...` (`OrchestrationRead`).
pub(crate) async fn handle_get_task_folder_get(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    Query(query): Query<WorktreePathQuery>,
) -> Result<Response, ServerRuntimeError> {
    let _principal = authorize_request(
        &state,
        &headers,
        &[EnvironmentScope::OrchestrationRead],
        false,
    )?;
    let Some(worktree_path) = query.worktree_path.filter(|s| !s.trim().is_empty()) else {
        return Err(ServerRuntimeError::BadRequest(
            "worktreePath query parameter is required".to_owned(),
        ));
    };
    let res =
        get_workspace_task_folder_service(&state, &WorkspaceTaskFolderInput { worktree_path })?;
    Ok((StatusCode::OK, Json(res)).into_response())
}

/// `POST /api/workspace/task-folders` (`OrchestrationRead`).
pub(crate) async fn handle_get_task_folder_post(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ServerRuntimeError> {
    let (_principal, input): (_, WorkspaceTaskFolderInput) = parse_authorized_json(
        &state,
        &headers,
        &[EnvironmentScope::OrchestrationRead],
        false,
        &body,
    )?;
    let res = get_workspace_task_folder_service(&state, &input)?;
    Ok((StatusCode::OK, Json(res)).into_response())
}

/// `POST /api/workspace/task-folders/repositories` (`OrchestrationOperate`).
pub(crate) async fn handle_add_task_folder_repository(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ServerRuntimeError> {
    let (_principal, input): (_, WorkspaceAddTaskFolderRepositoryInput) = parse_authorized_json(
        &state,
        &headers,
        &[EnvironmentScope::OrchestrationOperate],
        true,
        &body,
    )?;
    let task_folder = add_task_folder_repository_service(&state, &input)?;
    Ok((StatusCode::OK, Json(task_folder)).into_response())
}
