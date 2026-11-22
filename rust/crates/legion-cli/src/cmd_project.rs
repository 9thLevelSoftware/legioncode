//! Offline `legioncode project add|rename|remove` commands backed by `legion_persistence::project_store`.

use legion_persistence::ownership::{
    OwnershipError, ServerOwnerLock, clear_persisted_server_runtime_state,
    read_live_server_runtime_state,
};
use legion_persistence::paths::{ServerDerivedPaths, ensure_server_directories};
use legion_persistence::project_store::{
    CreateProjectInput, ProjectStoreError, add_or_unarchive_project, rename_project,
    soft_delete_project,
};

pub use crate::cmd_project_args::strip_windows_verbatim_prefix;
use crate::cmd_project_args::{
    ProjectAction, normalize_workspace_root, parse_project_cli, resolve_project_target,
};
use crate::fallback::{delegate_to_node, derive_paths, unit_forced_original, write_stdout_line};

/// Unit identifier for `RUST_ANYTHING_UNIT_PROJECT` and fallback logs.
pub const UNIT_PROJECT: &str = "PROJECT";

/// Runs `legioncode project`, handling offline, live-local, and `--remote-url` targets natively.
pub fn run(args: &[String]) -> i32 {
    if unit_forced_original(UNIT_PROJECT) {
        return delegate_to_node(UNIT_PROJECT, args);
    }

    let parsed = match parse_project_cli(args.get(1..).unwrap_or(&[])) {
        Ok(parsed) => parsed,
        Err(code) => return code,
    };

    let (_, paths) = derive_paths(parsed.base_dir.as_deref());
    let paths = if parsed.dev_url.is_some() {
        let (base_dir, is_explicit) = crate::fallback::resolve_base_dir(parsed.base_dir.as_deref());
        ServerDerivedPaths::derive(&base_dir, true, is_explicit)
    } else {
        paths
    };

    if let Some(ref remote_url) = parsed.remote_url {
        return crate::cmd_project_live::run_remote_server_action(
            remote_url,
            parsed.token.as_deref(),
            &paths,
            &parsed.action,
        );
    }

    if let Some(runtime) = read_live_server_runtime_state(&paths.server_runtime_state_path) {
        return crate::cmd_project_live::run_live_server_action(
            &paths,
            &runtime.origin,
            &parsed.action,
        );
    }

    if let Err(err) = ensure_server_directories(&paths) {
        eprintln!("Failed to prepare LegionCode directories: {err}");
        return 1;
    }

    let owner_lock = match ServerOwnerLock::hold_lock_only(&paths.server_runtime_state_path) {
        Ok(lock) => lock,
        Err(OwnershipError::StateDirOwned { .. }) => {
            return if let Some(runtime) =
                read_live_server_runtime_state(&paths.server_runtime_state_path)
            {
                crate::cmd_project_live::run_live_server_action(
                    &paths,
                    &runtime.origin,
                    &parsed.action,
                )
            } else {
                eprintln!("State directory is owned by a live server without runtime metadata.");
                1
            };
        }
        Err(err) => {
            eprintln!("Failed to acquire state directory ownership lock: {err}");
            return 1;
        }
    };

    if let Some(runtime) = read_live_server_runtime_state(&paths.server_runtime_state_path) {
        drop(owner_lock);
        return crate::cmd_project_live::run_live_server_action(
            &paths,
            &runtime.origin,
            &parsed.action,
        );
    }

    let exit_code = match parsed.action {
        ProjectAction::Add {
            workspace_root,
            title,
        } => run_add(&paths, &workspace_root, title.as_deref()),
        ProjectAction::Rename { project, title } => run_rename(&paths, &project, &title),
        ProjectAction::Remove { project, force } => run_remove(&paths, &project, force),
    };

    if exit_code == 0 {
        let _ = clear_persisted_server_runtime_state(&paths.server_runtime_state_path);
    }
    drop(owner_lock);
    exit_code
}

fn run_add(paths: &ServerDerivedPaths, raw_root: &str, explicit_title: Option<&str>) -> i32 {
    let ws_root = match normalize_workspace_root(raw_root, true) {
        Ok(root) => root,
        Err(msg) => {
            eprintln!("{msg}");
            return 1;
        }
    };

    let mut input = CreateProjectInput::new(ws_root.clone());
    if let Some(raw_title) = explicit_title {
        let trimmed = raw_title.trim();
        if trimmed.is_empty() {
            eprintln!("ProjectTitleEmptyError: Project title cannot be empty.");
            return 1;
        }
        input = input.with_title(trimmed);
    }

    match add_or_unarchive_project(&paths.db_path, &input) {
        Ok(res) if res.unarchived => write_stdout_line(
            &format!(
                "Unarchived project {} ({}) at {}.",
                res.project.id, res.project.title, res.project.workspace_root
            ),
            "project unarchive",
        ),
        Ok(res) => write_stdout_line(
            &format!(
                "Added project {} ({}) at {}.",
                res.project.id, res.project.title, res.project.workspace_root
            ),
            "project add",
        ),
        Err(ProjectStoreError::WorkspaceConflict { workspace_root, .. }) => {
            eprintln!(
                "ProjectAlreadyExistsError: An active project already exists for '{workspace_root}'."
            );
            1
        }
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}

fn run_rename(paths: &ServerDerivedPaths, identifier: &str, new_title: &str) -> i32 {
    let trimmed_id = identifier.trim();
    if trimmed_id.is_empty() {
        eprintln!("ProjectIdentifierEmptyError: Project identifier cannot be empty.");
        return 1;
    }
    let trimmed_title = new_title.trim();
    if trimmed_title.is_empty() {
        eprintln!("ProjectTitleEmptyError: Project title cannot be empty.");
        return 1;
    }

    let Some(target) = resolve_project_target(&paths.db_path, trimmed_id) else {
        eprintln!("ProjectNotFoundError: No active project found for '{trimmed_id}'.");
        return 1;
    };

    match rename_project(&paths.db_path, &target, trimmed_title) {
        Ok(res) if !res.changed => write_stdout_line(
            &format!(
                "Project {} is already named {}.",
                res.project.id, res.project.title
            ),
            "project rename",
        ),
        Ok(res) => write_stdout_line(
            &format!(
                "Renamed project {} to {}.",
                res.project.id, res.project.title
            ),
            "project rename",
        ),
        Err(ProjectStoreError::NotFound { .. }) => {
            eprintln!("ProjectNotFoundError: No active project found for '{trimmed_id}'.");
            1
        }
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}

fn run_remove(paths: &ServerDerivedPaths, identifier: &str, force: bool) -> i32 {
    let trimmed_id = identifier.trim();
    if trimmed_id.is_empty() {
        eprintln!("ProjectIdentifierEmptyError: Project identifier cannot be empty.");
        return 1;
    }

    let Some(target) = resolve_project_target(&paths.db_path, trimmed_id) else {
        eprintln!("ProjectNotFoundError: No active project found for '{trimmed_id}'.");
        return 1;
    };

    match soft_delete_project(&paths.db_path, &target, force) {
        Ok(res) => write_stdout_line(
            &format!(
                "Removed project {} ({}).",
                res.project.id, res.project.title
            ),
            "project remove",
        ),
        Err(ProjectStoreError::NotFound { .. }) => {
            eprintln!("ProjectNotFoundError: No active project found for '{trimmed_id}'.");
            1
        }
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}
