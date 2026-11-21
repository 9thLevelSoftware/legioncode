//! Live-server HTTP implementation for `t3 project add|rename|remove` when an active
//! server owns `<baseDir>` (zero Node delegation).

use std::time::Duration;

use serde_json::json;
use t3_persistence::paths::ServerDerivedPaths;
use t3_server_runtime::HttpProjectSnapshot;
use uuid::Uuid;

use crate::cmd_project_args::{ProjectAction, normalize_workspace_root};
use crate::cmd_project_http::{
    derive_default_title, fetch_snapshot, find_active_project, post_mutate, resolve_bearer_token,
};
use crate::fallback::write_stdout_line;

/// Executes `add|rename|remove` against a running local server via `/api/projects` and
/// `/api/projects/mutate` using a short-lived local pairing bearer session.
pub(crate) fn run_live_server_action(
    paths: &ServerDerivedPaths,
    origin: &str,
    action: &ProjectAction,
) -> i32 {
    let Ok(rt) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        eprintln!("Failed to initialize async runtime for `t3 project`.");
        return 1;
    };
    rt.block_on(execute_live_action(Some(paths), origin, None, action, true))
}

/// Executes `add|rename|remove` against a `--remote-url` server with an explicit or env bearer token.
pub(crate) fn run_remote_server_action(
    remote_url: &str,
    explicit_token: Option<&str>,
    paths: &ServerDerivedPaths,
    action: &ProjectAction,
) -> i32 {
    let Ok(rt) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        eprintln!("Failed to initialize async runtime for `t3 project`.");
        return 1;
    };
    let db_hint = paths.db_path.exists().then_some(paths);
    rt.block_on(execute_live_action(
        db_hint,
        remote_url,
        explicit_token,
        action,
        false,
    ))
}

async fn execute_live_action(
    paths: Option<&ServerDerivedPaths>,
    origin: &str,
    explicit_token: Option<&str>,
    action: &ProjectAction,
    require_dir: bool,
) -> i32 {
    if let Err(msg) = validate_local_inputs(action, require_dir) {
        eprintln!("{msg}");
        return 1;
    }

    let bearer = match resolve_bearer_token(paths, origin, explicit_token).await {
        Ok(tok) => tok,
        Err(msg) => {
            eprintln!("{msg}");
            return 1;
        }
    };
    let Ok(http) = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    else {
        eprintln!("Failed to construct HTTP client.");
        return 1;
    };

    let snapshot = match fetch_snapshot(&http, origin, &bearer).await {
        Ok(snap) => snap,
        Err(msg) => {
            eprintln!("{msg}");
            return 1;
        }
    };

    match action {
        ProjectAction::Add {
            workspace_root,
            title,
        } => {
            live_add(
                &http,
                origin,
                &bearer,
                &snapshot,
                workspace_root,
                title.as_deref(),
                require_dir,
            )
            .await
        }
        ProjectAction::Rename { project, title } => {
            live_rename(&http, origin, &bearer, &snapshot, project, title).await
        }
        ProjectAction::Remove { project, force } => {
            live_remove(&http, origin, &bearer, &snapshot, project, *force).await
        }
    }
}

fn validate_local_inputs(action: &ProjectAction, require_dir: bool) -> Result<(), String> {
    match action {
        ProjectAction::Add {
            workspace_root,
            title,
        } => {
            let _ = normalize_workspace_root(workspace_root, require_dir)?;
            if title.as_deref().is_some_and(|t| t.trim().is_empty()) {
                return Err("ProjectTitleEmptyError: Project title cannot be empty.".to_owned());
            }
            Ok(())
        }
        ProjectAction::Rename { project, title } => {
            if project.trim().is_empty() {
                return Err(
                    "ProjectIdentifierEmptyError: Project identifier cannot be empty.".to_owned(),
                );
            }
            if title.trim().is_empty() {
                return Err("ProjectTitleEmptyError: Project title cannot be empty.".to_owned());
            }
            Ok(())
        }
        ProjectAction::Remove { project, .. } => {
            if project.trim().is_empty() {
                return Err(
                    "ProjectIdentifierEmptyError: Project identifier cannot be empty.".to_owned(),
                );
            }
            Ok(())
        }
    }
}

async fn live_add(
    http: &reqwest::Client,
    origin: &str,
    bearer: &str,
    snapshot: &HttpProjectSnapshot,
    raw_root: &str,
    explicit_title: Option<&str>,
    require_dir: bool,
) -> i32 {
    let ws_root = match normalize_workspace_root(raw_root, require_dir) {
        Ok(root) => root,
        Err(msg) => {
            eprintln!("{msg}");
            return 1;
        }
    };
    let existing = snapshot
        .projects
        .iter()
        .find(|p| p.deleted_at.is_none() && p.workspace_root == ws_root);

    if let Some(proj) = existing {
        if proj.archived_at.is_some() {
            let mut body = json!({
                "type": "project.update",
                "projectId": proj.id,
                "archived": false,
            });
            let next_title = explicit_title.map(|t| t.trim().to_owned());
            if let (Some(t), Some(map)) = (&next_title, body.as_object_mut()) {
                map.insert("title".to_owned(), json!(t));
            }
            if let Err(msg) = post_mutate(http, origin, bearer, body).await {
                eprintln!("{msg}");
                return 1;
            }
            let shown_title = next_title.as_deref().unwrap_or(&proj.title);
            return write_stdout_line(
                &format!(
                    "Unarchived project {} ({shown_title}) at {ws_root}.",
                    proj.id
                ),
                "project unarchive",
            );
        }
        eprintln!("ProjectAlreadyExistsError: An active project already exists for '{ws_root}'.");
        return 1;
    }

    let resolved_title =
        explicit_title.map_or_else(|| derive_default_title(&ws_root), |t| t.trim().to_owned());
    let project_id = Uuid::new_v4().to_string();
    let body = json!({
        "type": "project.create",
        "projectId": project_id,
        "title": resolved_title,
        "workspaceRoot": ws_root,
    });
    if let Err(msg) = post_mutate(http, origin, bearer, body).await {
        eprintln!("{msg}");
        return 1;
    }
    write_stdout_line(
        &format!("Added project {project_id} ({resolved_title}) at {ws_root}."),
        "project add",
    )
}

async fn live_rename(
    http: &reqwest::Client,
    origin: &str,
    bearer: &str,
    snapshot: &HttpProjectSnapshot,
    identifier: &str,
    new_title: &str,
) -> i32 {
    let trimmed_id = identifier.trim();
    let trimmed_title = new_title.trim();
    let Some(proj) = find_active_project(snapshot, trimmed_id) else {
        eprintln!("ProjectNotFoundError: No active project found for '{trimmed_id}'.");
        return 1;
    };
    if proj.title == trimmed_title {
        return write_stdout_line(
            &format!("Project {} is already named {trimmed_title}.", proj.id),
            "project rename",
        );
    }
    let body = json!({
        "type": "project.update",
        "projectId": proj.id,
        "title": trimmed_title,
    });
    if let Err(msg) = post_mutate(http, origin, bearer, body).await {
        eprintln!("{msg}");
        return 1;
    }
    write_stdout_line(
        &format!("Renamed project {} to {trimmed_title}.", proj.id),
        "project rename",
    )
}

async fn live_remove(
    http: &reqwest::Client,
    origin: &str,
    bearer: &str,
    snapshot: &HttpProjectSnapshot,
    identifier: &str,
    force: bool,
) -> i32 {
    let trimmed_id = identifier.trim();
    let Some(proj) = find_active_project(snapshot, trimmed_id) else {
        eprintln!("ProjectNotFoundError: No active project found for '{trimmed_id}'.");
        return 1;
    };
    let body = json!({
        "type": "project.delete",
        "projectId": proj.id,
        "force": force,
    });
    if let Err(msg) = post_mutate(http, origin, bearer, body).await {
        eprintln!("{msg}");
        return 1;
    }
    write_stdout_line(
        &format!("Removed project {} ({}).", proj.id, proj.title),
        "project remove",
    )
}
