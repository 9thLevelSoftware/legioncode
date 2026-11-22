//! Native `legioncode theme` subcommands (`show`, `set`, `clear`) matching `apps/server/src/cli/theme.ts`.

use std::env;
use std::fs;
use std::io::{BufWriter, Write};
use std::path::Path;

use legion_persistence::ownership::read_live_server_runtime_state;
use legion_persistence::paths::ServerDerivedPaths;
use legion_remote::theme::{
    BUILT_IN_THEME_IDS, clear_default_theme, format_theme_cleared, format_theme_published_and_set,
    format_theme_set, format_theme_show, is_unpublishable_theme_id, is_valid_theme_id,
    publish_theme_file, read_and_validate_theme_file, read_default_theme, read_published_themes,
    set_default_theme,
};

use crate::cli_global::fail_with_command_help;
use crate::cmd_trace::current_epoch_ms;
use crate::fallback::{
    delegate_to_node, expand_home, resolve_base_dir, take_flag_value, unit_forced_original,
    write_stdout_line,
};

/// Unit identifier for `RUST_ANYTHING_UNIT_THEME` and fallback logs.
pub const UNIT_THEME: &str = "THEME";

const INVALID_THEME_ID_REASON: &str =
    "is not a valid theme id (lowercase letters, digits, and hyphens; not an appearance keyword)";

#[derive(Debug, Clone, PartialEq, Eq)]
struct ThemeSetArgs {
    base_dir: Option<String>,
    id: Option<String>,
    theme: String,
}

/// Runs `legioncode theme show|set|clear` natively unless `RUST_ANYTHING_UNIT_THEME=original`.
pub fn run(args: &[String]) -> i32 {
    if unit_forced_original(UNIT_THEME) {
        return delegate_to_node(UNIT_THEME, args);
    }

    let Some(subcmd) = args.get(1).map(String::as_str) else {
        return fail_with_command_help(&["theme"], "Missing subcommand for `legioncode theme`.");
    };
    let rest = args.get(2..).unwrap_or(&[]);

    match subcmd {
        "show" => match parse_base_dir_only(rest) {
            Ok(base_dir) => run_show(base_dir.as_deref()),
            Err(()) => fail_with_command_help(
                &["theme", "show"],
                "Invalid or unrecognized arguments for `legioncode theme show`.",
            ),
        },
        "clear" => match parse_base_dir_only(rest) {
            Ok(base_dir) => run_clear(base_dir.as_deref()),
            Err(()) => fail_with_command_help(
                &["theme", "clear"],
                "Invalid or unrecognized arguments for `legioncode theme clear`.",
            ),
        },
        "set" => match parse_set_args(rest) {
            Some(parsed) => run_set(&parsed),
            None => fail_with_command_help(
                &["theme", "set"],
                "Missing or invalid arguments for `legioncode theme set`.",
            ),
        },
        other => fail_with_command_help(
            &["theme"],
            &format!("Unknown subcommand \"{other}\" for \"legioncode theme\""),
        ),
    }
}

fn parse_base_dir_only(rest: &[String]) -> Result<Option<String>, ()> {
    let mut base_dir = None;
    let mut idx = 0;
    while idx < rest.len() {
        if let Ok(Some(val)) = take_flag_value(rest, &mut idx, "base-dir") {
            base_dir = Some(val);
        } else {
            return Err(());
        }
        idx += 1;
    }
    Ok(base_dir)
}

fn parse_set_args(rest: &[String]) -> Option<ThemeSetArgs> {
    let mut base_dir = None;
    let mut id = None;
    let mut theme = None;
    let mut idx = 0;

    while idx < rest.len() {
        let arg = &rest[idx];
        if let Ok(Some(val)) = take_flag_value(rest, &mut idx, "base-dir") {
            base_dir = Some(val);
        } else if let Ok(Some(val)) = take_flag_value(rest, &mut idx, "id") {
            id = Some(val);
        } else if arg.starts_with('-') || theme.is_some() {
            return None;
        } else {
            theme = Some(arg.clone());
        }
        idx += 1;
    }

    theme.map(|t| ThemeSetArgs {
        base_dir,
        id,
        theme: t,
    })
}

fn resolve_theme_paths(explicit_base_dir: Option<&str>) -> ServerDerivedPaths {
    let (base_dir, explicit) = resolve_base_dir(explicit_base_dir);
    let userdata = ServerDerivedPaths::derive(&base_dir, false, explicit);
    let configured = explicit_base_dir.is_some_and(|v| !v.trim().is_empty())
        || env::var("LEGIONCODE_HOME").is_ok_and(|v| !v.trim().is_empty());
    if !configured && read_live_server_runtime_state(&userdata.server_runtime_state_path).is_none()
    {
        let dev = ServerDerivedPaths::derive(&base_dir, true, false);
        if read_live_server_runtime_state(&dev.server_runtime_state_path).is_some() {
            return dev;
        }
    }
    userdata
}

fn run_show(base_dir: Option<&str>) -> i32 {
    let paths = resolve_theme_paths(base_dir);
    let default_theme = match read_default_theme(&paths.settings_path) {
        Ok(theme) => theme,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };
    let published_ids: Vec<String> = read_published_themes(&paths.environment_themes_dir)
        .into_iter()
        .map(|entry| entry.id)
        .collect();

    let rendered = format_theme_show(
        default_theme.as_deref(),
        &published_ids,
        &paths.environment_themes_dir,
    );
    let mut out = BufWriter::with_capacity(4096, std::io::stdout().lock());
    for line in rendered.lines() {
        if let Err(err) = writeln!(out, "{line}\n") {
            eprintln!("Failed to write theme status: {err}");
            return 1;
        }
    }
    if let Err(err) = out.flush() {
        eprintln!("Failed to flush theme output: {err}");
        return 1;
    }
    0
}

fn run_clear(base_dir: Option<&str>) -> i32 {
    let paths = resolve_theme_paths(base_dir);
    if let Err(err) = clear_default_theme(&paths.settings_path) {
        eprintln!("{err}");
        return 1;
    }
    write_stdout_line(&format_theme_cleared(&paths.settings_path), "theme output")
}

fn run_set(parsed: &ThemeSetArgs) -> i32 {
    let raw_trimmed = parsed.theme.trim();
    if raw_trimmed.is_empty() {
        eprintln!(
            "Provide a theme id or file, or run `legioncode theme clear` to remove the theme."
        );
        return 1;
    }

    let target = expand_home(raw_trimmed);
    let target_str = target.to_string_lossy();
    let looks_like_path = raw_trimmed.starts_with('~')
        || target_str.ends_with(".json")
        || target_str.contains('/')
        || target_str.contains('\\');

    let paths = resolve_theme_paths(parsed.base_dir.as_deref());
    let target_exists = looks_like_path && fs::symlink_metadata(&target).is_ok();

    if target_exists {
        return publish_and_set(&paths, &target, parsed.id.as_deref());
    }
    if looks_like_path {
        eprintln!("Could not read {}.", target.display());
        return 1;
    }

    let theme_id = target_str.as_ref();
    if !is_valid_theme_id(theme_id) {
        eprintln!("\"{theme_id}\" {INVALID_THEME_ID_REASON}.");
        return 1;
    }

    let mut known: Vec<String> = BUILT_IN_THEME_IDS.iter().map(|s| (*s).to_owned()).collect();
    for entry in read_published_themes(&paths.environment_themes_dir) {
        known.push(entry.id);
    }
    known.sort();
    known.dedup();
    if !known.iter().any(|candidate| candidate == theme_id) {
        eprintln!(
            "No theme named \"{theme_id}\". Available: {}. Publish one by passing a theme file instead of an id.",
            known.join(", ")
        );
        return 1;
    }

    if let Err(err) = set_default_theme(&paths.settings_path, theme_id, current_epoch_ms()) {
        eprintln!("{err}");
        return 1;
    }
    write_stdout_line(
        &format_theme_set(theme_id, &paths.settings_path),
        "theme output",
    )
}

fn publish_and_set(paths: &ServerDerivedPaths, target: &Path, explicit_id: Option<&str>) -> i32 {
    if let Err(err) = read_default_theme(&paths.settings_path) {
        eprintln!("{err}");
        return 1;
    }

    let (raw_bytes, _parsed) = match read_and_validate_theme_file(target) {
        Ok(res) => res,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };

    let derived_id = explicit_id.map_or_else(|| basename_without_json(target), str::to_owned);
    if !is_valid_theme_id(&derived_id) || is_unpublishable_theme_id(&derived_id) {
        eprintln!("\"{derived_id}\" {INVALID_THEME_ID_REASON}. Pass one with --id.");
        return 1;
    }

    let handle = match publish_theme_file(&paths.environment_themes_dir, &derived_id, &raw_bytes) {
        Ok(handle) => handle,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };

    if let Err(err) = set_default_theme(&paths.settings_path, &derived_id, current_epoch_ms()) {
        let _ = handle.revert();
        eprintln!("{err}");
        return 1;
    }
    let _ = handle.cleanup();

    write_stdout_line(
        &format_theme_published_and_set(target, &derived_id),
        "theme output",
    )
}

fn basename_without_json(path: &Path) -> String {
    let file_name = path
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    file_name
        .strip_suffix(".json")
        .unwrap_or(&file_name)
        .to_owned()
}
