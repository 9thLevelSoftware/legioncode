//! Argument parsing and workspace-root normalization helpers for `t3 project`.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use t3_persistence::project_store::find_project_by_id_or_workspace;

use crate::cli_global::fail_with_command_help;
use crate::fallback::{expand_home, resolve_platform_path, take_flag_value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedProjectCli {
    pub(crate) base_dir: Option<String>,
    pub(crate) dev_url: Option<String>,
    pub(crate) remote_url: Option<String>,
    pub(crate) token: Option<String>,
    pub(crate) action: ProjectAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProjectAction {
    Add {
        workspace_root: String,
        title: Option<String>,
    },
    Rename {
        project: String,
        title: String,
    },
    Remove {
        project: String,
        force: bool,
    },
}

pub(crate) fn parse_project_cli(rest: &[String]) -> Result<ParsedProjectCli, i32> {
    let Some(subcmd) = rest.first().map(String::as_str) else {
        return Err(fail_with_command_help(
            &["project"],
            "Missing subcommand for `t3 project`.",
        ));
    };
    let tail = rest.get(1..).unwrap_or(&[]);
    match subcmd {
        "add" => parse_subcommand(tail, &["project", "add"], ProjectSub::Add),
        "rename" => parse_subcommand(tail, &["project", "rename"], ProjectSub::Rename),
        "remove" => parse_subcommand(tail, &["project", "remove"], ProjectSub::Remove),
        other => Err(fail_with_command_help(
            &["project"],
            &format!("Unknown subcommand \"{other}\" for \"t3 project\""),
        )),
    }
}

#[derive(Clone, Copy)]
enum ProjectSub {
    Add,
    Rename,
    Remove,
}

fn parse_subcommand(
    tail: &[String],
    path: &[&str],
    sub: ProjectSub,
) -> Result<ParsedProjectCli, i32> {
    let mut base_dir = None;
    let mut dev_url = None;
    let mut remote_url = None;
    let mut token = None;
    let mut title = None;
    let mut force = false;
    let mut pos = Vec::new();
    let mut idx = 0;

    while idx < tail.len() {
        let arg = &tail[idx];
        if matches!(sub, ProjectSub::Remove) && arg == "--force" {
            force = true;
        } else if let Some(val) = take_proj_opt(path, tail, &mut idx, "base-dir")? {
            base_dir = Some(val);
        } else if let Some(val) = take_proj_opt(path, tail, &mut idx, "dev-url")? {
            validate_url(path, "--dev-url", &val)?;
            dev_url = Some(val);
        } else if let Some(val) = take_proj_opt(path, tail, &mut idx, "remote-url")? {
            validate_url(path, "--remote-url", &val)?;
            remote_url = Some(val);
        } else if let Some(val) = take_proj_opt(path, tail, &mut idx, "token")? {
            token = Some(val);
        } else if matches!(sub, ProjectSub::Add)
            && let Some(val) = take_proj_opt(path, tail, &mut idx, "title")?
        {
            title = Some(val);
        } else if arg.starts_with('-') {
            return Err(fail_with_command_help(
                path,
                &format!("Unrecognized flag: {arg} in command t3 {}", path.join(" ")),
            ));
        } else {
            pos.push(arg.clone());
        }
        idx += 1;
    }

    let action = finalize_project_action(sub, path, pos, title, force)?;
    Ok(ParsedProjectCli {
        base_dir,
        dev_url,
        remote_url,
        token,
        action,
    })
}

fn finalize_project_action(
    sub: ProjectSub,
    path: &[&str],
    mut pos: Vec<String>,
    title: Option<String>,
    force: bool,
) -> Result<ProjectAction, i32> {
    match sub {
        ProjectSub::Add => {
            if pos.is_empty() {
                return Err(fail_with_command_help(
                    path,
                    "Missing required argument: path",
                ));
            }
            if pos.len() > 1 {
                return Err(fail_with_command_help(
                    path,
                    &format!("Unexpected argument: {}", pos[1]),
                ));
            }
            Ok(ProjectAction::Add {
                workspace_root: pos.swap_remove(0),
                title,
            })
        }
        ProjectSub::Rename => {
            if pos.is_empty() {
                return Err(fail_with_command_help(
                    path,
                    "Missing required argument: project",
                ));
            }
            if pos.len() < 2 {
                return Err(fail_with_command_help(
                    path,
                    "Missing required argument: title",
                ));
            }
            if pos.len() > 2 {
                return Err(fail_with_command_help(
                    path,
                    &format!("Unexpected argument: {}", pos[2]),
                ));
            }
            let new_title = pos.swap_remove(1);
            let project = pos.swap_remove(0);
            Ok(ProjectAction::Rename {
                project,
                title: new_title,
            })
        }
        ProjectSub::Remove => {
            if pos.is_empty() {
                return Err(fail_with_command_help(
                    path,
                    "Missing required argument: project",
                ));
            }
            if pos.len() > 1 {
                return Err(fail_with_command_help(
                    path,
                    &format!("Unexpected argument: {}", pos[1]),
                ));
            }
            Ok(ProjectAction::Remove {
                project: pos.swap_remove(0),
                force,
            })
        }
    }
}

fn take_proj_opt(
    path: &[&str],
    tail: &[String],
    idx: &mut usize,
    name: &str,
) -> Result<Option<String>, i32> {
    take_flag_value(tail, idx, name)
        .map_err(|()| fail_with_command_help(path, &format!("Missing value for flag --{name}")))
}

fn validate_url(path: &[&str], flag: &str, raw: &str) -> Result<(), i32> {
    if raw.starts_with("http://") || raw.starts_with("https://") {
        Ok(())
    } else {
        Err(fail_with_command_help(
            path,
            &format!(
                "Invalid value for flag {flag}: \"{raw}\". Expected an http:// or https:// URL."
            ),
        ))
    }
}

pub(crate) fn resolve_project_target(db_path: &Path, trimmed_id: &str) -> Option<String> {
    if !db_path.exists() {
        return None;
    }
    if let Ok(Some(record)) = find_project_by_id_or_workspace(db_path, trimmed_id) {
        return Some(record.id);
    }
    let normalized = normalize_workspace_root(trimmed_id, false).ok()?;
    if let Ok(Some(record)) = find_project_by_id_or_workspace(db_path, &normalized) {
        return Some(record.id);
    }
    None
}

/// Normalizes a workspace root path lexically and strips Windows `\\?\` verbatim prefixes.
///
/// ```
/// #[cfg(windows)]
/// {
///     let norm = t3_cli::cmd_project::strip_windows_verbatim_prefix(r"\\?\D:\t3code\apps\");
///     assert_eq!(norm, r"D:\t3code\apps\");
/// }
/// ```
pub fn strip_windows_verbatim_prefix(raw: &str) -> String {
    #[cfg(windows)]
    {
        if let Some(unc) = raw.strip_prefix(r"\\?\UNC\") {
            return format!(r"\\{unc}");
        }
        if let Some(local) = raw.strip_prefix(r"\\?\") {
            return local.to_owned();
        }
    }
    raw.to_owned()
}

pub(crate) fn normalize_workspace_root(raw: &str, require_dir: bool) -> Result<String, String> {
    let clean_raw = strip_windows_verbatim_prefix(raw.trim());
    let expanded = expand_home(&clean_raw);
    let resolved = resolve_platform_path(&expanded);
    let normalized = strip_windows_verbatim_prefix(&resolved.to_string_lossy());

    if require_dir {
        match fs::metadata(&normalized) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => {
                return Err(format!(
                    "WorkspaceRootNotDirectoryError: Workspace root is not a directory: {normalized}"
                ));
            }
            Err(err) if err.kind() == ErrorKind::NotFound => {
                return Err(format!(
                    "WorkspaceRootNotExistsError: Workspace root does not exist: {normalized}"
                ));
            }
            Err(err) => {
                return Err(format!(
                    "WorkspaceRootStatFailedError: Failed to stat workspace root '{normalized}': {err}"
                ));
            }
        }
    }

    Ok(normalized)
}
