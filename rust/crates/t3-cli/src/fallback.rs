//! Legacy Node fallback delegation, environment overrides, and `base_dir` derivation.

use std::env;
use std::fs::{self, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Serialize;
use t3_persistence::paths::ServerDerivedPaths;

/// Absolute path to the legacy TypeScript CLI entrypoint invoked on fallback.
pub const LEGACY_BIN_TS: &str = "d:/t3code/apps/server/src/bin.ts";

const FALLBACK_LOG_ENV: &str = "RUST_ANYTHING_FALLBACK_LOG";
const UNIT_FORCE_PREFIX: &str = "RUST_ANYTHING_UNIT_";

#[derive(Serialize)]
struct FallbackLogEntry<'a> {
    unit: &'a str,
    args: &'a [String],
}

/// Returns `true` when `RUST_ANYTHING_UNIT_<UNIT>` is set to `"original"` (ASCII case-insensitive).
///
/// ```
/// assert!(!t3_cli::fallback::unit_forced_original("UNSET_UNIT_FOR_DOCTEST"));
/// ```
pub fn unit_forced_original(unit: &str) -> bool {
    let key = format!("{UNIT_FORCE_PREFIX}{}", unit.to_ascii_uppercase());
    env::var(key).is_ok_and(|val| val.trim().eq_ignore_ascii_case("original"))
}

/// Logs fallback invocation to `RUST_ANYTHING_FALLBACK_LOG` (when set and non-empty) and delegates
/// execution to `node <T3_BIN_TS or LEGACY_BIN_TS> ...`.
pub fn delegate_to_node(unit: &str, passthrough_args: &[String]) -> i32 {
    if let Ok(log_path) = env::var(FALLBACK_LOG_ENV) {
        let trimmed = log_path.trim();
        if !trimmed.is_empty() {
            let _ = append_fallback_log(Path::new(trimmed), unit, passthrough_args);
        }
    }

    let bin_ts = env::var("T3_BIN_TS")
        .ok()
        .map(|val| val.trim().to_owned())
        .filter(|val| !val.is_empty())
        .unwrap_or_else(|| LEGACY_BIN_TS.to_owned());

    match Command::new("node")
        .arg(bin_ts)
        .args(passthrough_args)
        .env("T3CODE_LEGACY_NODE", "1")
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
    {
        Ok(status) => status.code().unwrap_or(1),
        Err(err) => {
            eprintln!("Failed to spawn node for fallback unit {unit}: {err}");
            1
        }
    }
}

fn append_fallback_log(
    path: &Path,
    unit: &str,
    passthrough_args: &[String],
) -> std::io::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    let line = serde_json::to_string(&FallbackLogEntry {
        unit,
        args: passthrough_args,
    })
    .map_err(std::io::Error::other)?;
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{line}")
}

/// Resolves the T3 Code `base_dir` and whether it was explicitly specified (`--base-dir` or `T3CODE_HOME`).
///
/// Order:
/// 1. `--base-dir <dir>` -> normalized `(path, true)` matching Node `Path.resolve`
/// 2. Non-empty `T3CODE_HOME` -> normalized `(path, true)` matching Node `Path.resolve`
/// 3. `(<USERPROFILE or HOME>/.t3, false)`
///
/// ```
/// #[cfg(windows)]
/// {
///     let (resolved, explicit) = t3_cli::fallback::resolve_base_dir(Some("D:/tmp//t3home/"));
///     assert!(explicit);
///     assert_eq!(resolved, std::path::PathBuf::from(r"D:\tmp\t3home"));
/// }
/// #[cfg(not(windows))]
/// {
///     let (resolved, explicit) = t3_cli::fallback::resolve_base_dir(Some("/tmp//t3home/"));
///     assert!(explicit);
///     assert_eq!(resolved, std::path::PathBuf::from("/tmp/t3home"));
/// }
/// ```
pub fn resolve_base_dir(explicit: Option<&str>) -> (PathBuf, bool) {
    if let Some(dir) = explicit {
        let trimmed = dir.trim();
        if !trimmed.is_empty() {
            return (resolve_platform_path(&expand_home(trimmed)), true);
        }
    }
    if let Ok(val) = env::var("T3CODE_HOME") {
        let trimmed = val.trim();
        if !trimmed.is_empty() {
            return (resolve_platform_path(&expand_home(trimmed)), true);
        }
    }
    (resolve_platform_path(&user_home_dir().join(".t3")), false)
}

/// Derives `base_dir` and [`ServerDerivedPaths`] for CLI commands using `dev_url_present = false`.
pub fn derive_paths(explicit: Option<&str>) -> (PathBuf, ServerDerivedPaths) {
    let (base_dir, is_explicit) = resolve_base_dir(explicit);
    let paths = ServerDerivedPaths::derive(&base_dir, false, is_explicit);
    (base_dir, paths)
}

/// Expands a leading `~`, `~/`, or `~\` against the user's home directory.
///
/// Non-tilde inputs are returned verbatim so callers like `t3 policy check <file>` or
/// `t3 theme set <file>` preserve CLI display paths while tilde paths normalize separators
/// on Windows (`~/a/b.json` -> `<home>\a\b.json`).
pub fn expand_home(raw: &str) -> PathBuf {
    if raw == "~" {
        return lexical_normalize(&to_platform_separators(&user_home_dir()));
    }
    if let Some(rest) = raw.strip_prefix("~/").or_else(|| raw.strip_prefix("~\\")) {
        let joined = user_home_dir().join(rest);
        return lexical_normalize(&to_platform_separators(&joined));
    }
    PathBuf::from(raw)
}

fn to_platform_separators(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from(path.to_string_lossy().replace('/', "\\"))
    }
    #[cfg(not(windows))]
    {
        path.to_path_buf()
    }
}

pub(crate) fn resolve_platform_path(path: &Path) -> PathBuf {
    let replaced = to_platform_separators(path);
    let abs = std::path::absolute(&replaced).unwrap_or(replaced);
    lexical_normalize(&abs)
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut stack: Vec<Component<'_>> = Vec::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(stack.last(), Some(Component::Normal(_))) {
                    stack.pop();
                } else if !matches!(
                    stack.last(),
                    Some(Component::RootDir | Component::Prefix(_))
                ) {
                    stack.push(comp);
                }
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                stack.push(comp);
            }
        }
    }
    if stack.is_empty() {
        PathBuf::from(".")
    } else {
        stack.iter().collect()
    }
}

pub(crate) fn user_home_dir() -> PathBuf {
    #[cfg(windows)]
    let keys = ["USERPROFILE", "HOME"];
    #[cfg(not(windows))]
    let keys = ["HOME", "USERPROFILE"];

    for key in keys {
        if let Ok(val) = env::var(key) {
            let trimmed = val.trim();
            if !trimmed.is_empty() {
                return PathBuf::from(trimmed);
            }
        }
    }
    PathBuf::from(".")
}

/// Parses `--flag <value>` or `--flag=<value>` from `args` at index `idx`.
///
/// Advances `idx` when a separate following token is consumed. Returns `Err(())` if the flag
/// matches `--<flag_name>` without a value token.
pub(crate) fn take_flag_value(
    args: &[String],
    idx: &mut usize,
    flag_name: &str,
) -> Result<Option<String>, ()> {
    let current = &args[*idx];
    let long = format!("--{flag_name}");
    if current == &long {
        let Some(next) = args.get(*idx + 1) else {
            return Err(());
        };
        *idx += 1;
        return Ok(Some(next.clone()));
    }
    let prefix = format!("{long}=");
    if let Some(val) = current.strip_prefix(&prefix) {
        return Ok(Some(val.to_owned()));
    }
    Ok(None)
}

pub(crate) fn write_stdout_line(text: &str, context: &str) -> i32 {
    let mut out = BufWriter::with_capacity(8192, std::io::stdout().lock());
    if let Err(err) = writeln!(out, "{text}").and_then(|()| out.flush()) {
        eprintln!("Failed to write {context}: {err}");
        return 1;
    }
    0
}

pub(crate) fn write_stdout_raw(text: &str, context: &str) -> i32 {
    let mut out = BufWriter::with_capacity(8192, std::io::stdout().lock());
    if let Err(err) = out.write_all(text.as_bytes()).and_then(|()| out.flush()) {
        eprintln!("Failed to write {context}: {err}");
        return 1;
    }
    0
}
