//! `t3 install` standalone runtime launcher installer (`cli/install.ts`).

use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::cli_global::fail_with_command_help;
use crate::fallback::{
    delegate_to_node, resolve_platform_path, unit_forced_original, user_home_dir, write_stdout_line,
};
use crate::invocation::{non_empty_env, resolve_command_on_path};

/// Unit identifier for `RUST_ANYTHING_UNIT_INSTALL` and fallback logs.
pub const UNIT_INSTALL: &str = "INSTALL";

/// Runs `t3 install` natively unless `RUST_ANYTHING_UNIT_INSTALL=original` is set.
pub fn run(args: &[String]) -> i32 {
    if unit_forced_original(UNIT_INSTALL) {
        return delegate_to_node(UNIT_INSTALL, args);
    }

    if let Some(unexpected) = args.get(1) {
        let msg = if unexpected.starts_with('-') {
            format!("Unrecognized flag: {unexpected} in command t3 install")
        } else {
            format!("Unexpected argument: {unexpected}")
        };
        return fail_with_command_help(&["install"], &msg);
    }

    let Some((executable, versions_dir, runtime_dir)) = detect_standalone_runtime() else {
        let script = if cfg!(windows) {
            "irm https://t3.codes/install.ps1 | iex"
        } else {
            "curl -fsSL https://t3.codes/install.sh | sh"
        };
        eprintln!(
            "CliInstallError: Only the standalone t3 can install itself. Use `{script}`, or Settings in the desktop app."
        );
        return 1;
    };

    if cfg!(windows) {
        let s = executable.to_string_lossy();
        if s.contains('%') || !s.is_ascii() {
            eprintln!(
                "CliInstallError: A t3.cmd cannot run {s}, because the path has % or non-ASCII characters. Keep using npx t3."
            );
            return 1;
        }
    }

    let bin_dir = resolve_install_bin_dir();
    let launcher_name = if cfg!(windows) { "t3.cmd" } else { "t3" };
    let link = bin_dir.join(launcher_name);

    let owned_target = read_existing_launcher_target(&link, &bin_dir);
    let occupied = fs::symlink_metadata(&link).is_ok();
    if occupied
        && !is_owned_by_runtime(
            owned_target.as_deref(),
            &versions_dir,
            &runtime_dir,
            launcher_name,
        )
    {
        eprintln!(
            "CliInstallError: Another t3 is already at {}. Remove it, then run this again.",
            link.display()
        );
        return 1;
    }

    if let Err(err) = fs::create_dir_all(&bin_dir) {
        eprintln!(
            "CliInstallError: Could not create {}: {err}",
            bin_dir.display()
        );
        return 1;
    }

    if let Err(err) = write_launcher_file(&link, &executable) {
        eprintln!("CliInstallError: Could not write {}: {err}", link.display());
        return 1;
    }

    let canonical_bin = fs::canonicalize(&bin_dir).unwrap_or_else(|_| bin_dir.clone());
    let recorded = canonical_bin.join(launcher_name);
    let _ = record_launcher(&runtime_dir, &recorded);

    let on_path = dir_on_path(&bin_dir);
    let shadowed_by = find_shadowing_t3(&bin_dir);
    let version = env!("CARGO_PKG_VERSION");

    println!("Installed t3 {version} at {}.", link.display());
    if let Some(other) = shadowed_by {
        println!(
            "Warning: `{}` comes first on PATH, so `t3` runs that instead. Reorder PATH or remove it, then run `t3`.",
            other.display()
        );
        return 0;
    }
    if on_path {
        return write_stdout_line("Run `t3` to get started.", "install");
    }
    write_stdout_line(
        &format!("Add {} to your PATH, then run `t3`.", bin_dir.display()),
        "install",
    )
}

pub(crate) fn detect_standalone_runtime() -> Option<(PathBuf, PathBuf, PathBuf)> {
    let exe = crate::invocation::resolve_install_launcher()?;
    let versions_dir = exe.parent()?.parent()?.to_path_buf();
    let runtime_dir = versions_dir.parent()?.to_path_buf();
    if versions_dir.file_name()?.to_str() != Some("versions")
        || runtime_dir.file_name()?.to_str() != Some("runtime")
    {
        return None;
    }
    Some((exe, versions_dir, runtime_dir))
}

pub(crate) fn is_version_executable(versions_dir: &Path, candidate: &Path) -> bool {
    let abs_versions = resolve_platform_path(versions_dir);
    let abs_cand = resolve_platform_path(candidate);
    let Ok(rel) = abs_cand.strip_prefix(&abs_versions) else {
        return false;
    };
    let parts: Vec<_> = rel.components().collect();
    if parts.len() != 2 {
        return false;
    }
    let name = parts[1].as_os_str().to_string_lossy();
    name.eq_ignore_ascii_case("t3") || name.eq_ignore_ascii_case("t3.exe")
}

#[cfg(windows)]
pub(crate) fn extract_windows_shim_target(contents: &str) -> Option<&str> {
    let line = contents
        .lines()
        .find(|l| !l.trim().is_empty() && !l.starts_with("@echo off"))?;
    let trimmed = line.trim();
    let rest = trimmed.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(&rest[..end])
}

#[cfg(not(windows))]
pub(crate) fn extract_posix_launcher_target(contents: &str) -> Option<String> {
    let line = contents.strip_prefix("#!/bin/sh\nexec '")?;
    let end = line.rfind("' \"$@\"\n")?;
    Some(line[..end].replace("'\\''", "'"))
}

pub(crate) fn write_launcher_file(link: &Path, executable: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        let content = format!("@echo off\r\n\"{}\" %*", executable.display());
        fs::write(link, content)
    }
    #[cfg(not(windows))]
    {
        let escaped = executable.to_string_lossy().replace('\'', "'\\''");
        let content = format!("#!/bin/sh\nexec '{escaped}' \"$@\"\n");
        let parent = link.parent().unwrap_or_else(|| Path::new("."));
        let tmp = parent.join(format!(".t3-{}", std::process::id()));
        fs::write(&tmp, content)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755))?;
        }
        fs::rename(&tmp, link)
    }
}

fn resolve_install_bin_dir() -> PathBuf {
    non_empty_env("T3CODE_INSTALL_BIN_DIR")
        .map_or_else(|| user_home_dir().join(".local").join("bin"), PathBuf::from)
        .pipe_resolve()
}

trait ResolveExt {
    fn pipe_resolve(self) -> PathBuf;
}

impl ResolveExt for PathBuf {
    fn pipe_resolve(self) -> PathBuf {
        resolve_platform_path(&self)
    }
}

fn read_existing_launcher_target(link: &Path, _bin_dir: &Path) -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let raw = fs::read_to_string(link).ok()?;
        extract_windows_shim_target(&raw).map(PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        if let Ok(target) = fs::read_link(link) {
            return Some(fs::canonicalize(link).unwrap_or_else(|_| _bin_dir.join(target)));
        }
        let raw = fs::read_to_string(link).ok()?;
        extract_posix_launcher_target(&raw).map(PathBuf::from)
    }
}

fn is_owned_by_runtime(
    target: Option<&Path>,
    versions_dir: &Path,
    runtime_dir: &Path,
    launcher_name: &str,
) -> bool {
    let Some(t) = target else {
        return false;
    };
    let desktop_bin = runtime_dir
        .parent()
        .map(|h| h.join("bin").join(launcher_name));
    is_version_executable(versions_dir, t) || desktop_bin.as_deref() == Some(t)
}

fn record_launcher(runtime_dir: &Path, launcher: &Path) -> std::io::Result<()> {
    let path = runtime_dir.join("launchers");
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let target = launcher.to_string_lossy();
    if existing.lines().any(|l| l == target.as_ref()) {
        return Ok(());
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{target}")
}

fn dir_on_path(bin_dir: &Path) -> bool {
    let sep = if cfg!(windows) { ';' } else { ':' };
    let target = resolve_platform_path(bin_dir);
    env::var("PATH")
        .unwrap_or_default()
        .split(sep)
        .any(|entry| {
            !entry.trim().is_empty() && paths_eq(&resolve_platform_path(Path::new(entry)), &target)
        })
}

fn find_shadowing_t3(bin_dir: &Path) -> Option<PathBuf> {
    let found = resolve_command_on_path("t3")?;
    let parent = found.parent()?;
    if paths_eq(
        &resolve_platform_path(parent),
        &resolve_platform_path(bin_dir),
    ) {
        None
    } else {
        Some(found)
    }
}

fn paths_eq(a: &Path, b: &Path) -> bool {
    #[cfg(windows)]
    {
        a.to_string_lossy()
            .eq_ignore_ascii_case(&b.to_string_lossy())
    }
    #[cfg(not(windows))]
    {
        a == b
    }
}
