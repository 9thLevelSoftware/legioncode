//! Host CLI invocation resolution and shell-word quoting matching `apps/server/src/cli/invocation.ts`.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// Returns `true` when cloud public config indicates Legion Connect hints are active.
pub(crate) fn has_cloud_public_config() -> bool {
    if env::var("LEGIONCODE_CLOUD_PUBLIC_CONFIG").is_ok_and(|val| !val.trim().is_empty()) {
        return true;
    }
    let has_relay =
        env::var("LEGIONCODE_RELAY_URL").is_ok_and(|val| val.trim().starts_with("https://"));
    let has_clerk_pk =
        env::var("LEGIONCODE_CLERK_PUBLISHABLE_KEY").is_ok_and(|val| !val.trim().is_empty());
    let has_clerk_client =
        env::var("LEGIONCODE_CLERK_CLI_OAUTH_CLIENT_ID").is_ok_and(|val| !val.trim().is_empty());
    has_relay && has_clerk_pk && has_clerk_client
}

/// Quotes a CLI argument for POSIX shells matching `invocation.ts:shellWord`.
///
/// ```
/// assert_eq!(legion_cli::invocation::shell_word("--base-dir"), "--base-dir");
/// assert_eq!(legion_cli::invocation::shell_word("a b"), "'a b'");
/// assert_eq!(legion_cli::invocation::shell_word("a'b"), r#"'a'"'"'b'"#);
/// ```
pub fn shell_word(value: &str) -> String {
    let bare = !value.is_empty()
        && value.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(
                    b,
                    b'_' | b'@' | b'%' | b'+' | b'=' | b':' | b',' | b'.' | b'/' | b'-'
                )
        });
    if bare {
        value.to_owned()
    } else {
        format!("'{}'", value.replace('\'', "'\"'\"'"))
    }
}

/// Quotes a CLI argument for PowerShell matching `invocation.ts:powershellWord`.
///
/// ```
/// assert_eq!(legion_cli::invocation::powershell_word("--base-dir"), "\"--base-dir\"");
/// assert_eq!(legion_cli::invocation::powershell_word("a`$b\"c"), "\"a`` `$b\"\"c\"".replace(' ', ""));
/// ```
pub fn powershell_word(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 4);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '`' => out.push_str("``"),
            '$' => out.push_str("`$"),
            '"' | '\u{201C}' | '\u{201D}' | '\u{201E}' => {
                out.push(ch);
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

/// Quotes an argument using the host shell syntax (`powershellWord` on Windows, `shellWord` elsewhere).
pub(crate) fn host_shell_word(value: &str) -> String {
    #[cfg(windows)]
    {
        powershell_word(value)
    }
    #[cfg(not(windows))]
    {
        shell_word(value)
    }
}

/// Resolves the CLI command hint string (`legioncode <sub> ...` or launcher path) matching `resolveHostCliCommand`.
pub(crate) fn resolve_host_cli_command(subcommand: &str, args: &[String]) -> String {
    let base_command = detect_cli_command(subcommand);
    if base_command != format!("legioncode {subcommand}") {
        return with_args(&base_command, args);
    }

    if let Some(on_path) = resolve_command_on_path("legioncode") {
        let own = resolve_own_cli_target();
        if own.is_none() || own == fs::canonicalize(&on_path).ok() {
            return with_args(&base_command, args);
        }
    }

    if let Some(launcher) = resolve_install_launcher() {
        let quoted = host_shell_word(&launcher.to_string_lossy());
        #[cfg(windows)]
        let call = format!("& {quoted}");
        #[cfg(not(windows))]
        let call = quoted;
        return with_args(&format!("{call} {subcommand}"), args);
    }

    with_args(&base_command, args)
}

fn with_args(prefix: &str, args: &[String]) -> String {
    let mut parts = Vec::with_capacity(1 + args.len());
    parts.push(prefix.trim_end().to_owned());
    for arg in args {
        parts.push(host_shell_word(arg));
    }
    parts.join(" ")
}

fn detect_cli_command(subcommand: &str) -> String {
    let entry_path = env::var("LEGIONCODE_NPM_LAUNCHER_PATH")
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            env::current_exe()
                .ok()
                .map(|p| p.to_string_lossy().into_owned())
        });

    if let Some(entry) = entry_path
        && let Some(runner) = detect_cli_runner(&entry)
    {
        let pkg_spec = suggested_package_spec(env!("CARGO_PKG_VERSION"));
        return format!("{runner} {pkg_spec} {subcommand}");
    }
    format!("legioncode {subcommand}")
}

fn detect_cli_runner(entry_path: &str) -> Option<&'static str> {
    let normalized = entry_path.replace('\\', "/");
    if normalized.contains("/_npx/") {
        Some("npx")
    } else if normalized.contains("/pnpm/dlx/")
        || normalized.contains("/.pnpm/dlx/")
        || normalized.contains("/pnpm-cache/dlx/")
    {
        Some("pnpm dlx")
    } else if normalized.contains("/.bun/install/cache/") || normalized.contains("/bunx-") {
        Some("bunx")
    } else {
        None
    }
}

fn suggested_package_spec(version: &str) -> &'static str {
    if version.contains("-nightly.") {
        "legioncode@nightly"
    } else if version.contains("-preview.") {
        "legioncode@preview"
    } else {
        "legioncode"
    }
}

pub(crate) fn resolve_own_cli_target() -> Option<PathBuf> {
    if let Some(shim) = non_empty_env("LEGIONCODE_CLI_PATH") {
        return fs::canonicalize(shim).ok();
    }
    #[cfg(windows)]
    {
        None
    }
    #[cfg(not(windows))]
    {
        env::current_exe()
            .ok()
            .and_then(|p| fs::canonicalize(p).ok())
    }
}

pub(crate) fn resolve_install_launcher() -> Option<PathBuf> {
    if let Some(shim) = non_empty_env("LEGIONCODE_CLI_PATH") {
        let candidate = PathBuf::from(&shim);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    env::current_exe().ok()
}

pub(crate) fn non_empty_env(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

pub(crate) fn resolve_command_on_path(command: &str) -> Option<PathBuf> {
    let path_var = env::var("PATH").ok()?;
    if path_var.trim().is_empty() {
        return None;
    }

    #[cfg(windows)]
    {
        let pathext = non_empty_env("PATHEXT").unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".to_owned());
        let exts: Vec<String> = pathext
            .split(';')
            .map(|e| e.trim().to_ascii_uppercase())
            .filter(|e| !e.is_empty())
            .collect();
        let candidates = windows_candidates(command, &exts);
        for raw_dir in path_var.split(';') {
            let dir = normalize_path_entry(raw_dir);
            if dir.is_empty() {
                continue;
            }
            for cand in &candidates {
                let path = Path::new(dir).join(cand);
                if is_executable_file(&path, &exts) {
                    return Some(path);
                }
            }
        }
        None
    }
    #[cfg(not(windows))]
    {
        for raw_dir in path_var.split(':') {
            let dir = normalize_path_entry(raw_dir);
            if dir.is_empty() {
                continue;
            }
            let path = Path::new(dir).join(command);
            if is_executable_file(&path, &[]) {
                return Some(path);
            }
        }
        None
    }
}

#[cfg(windows)]
fn windows_candidates(command: &str, exts: &[String]) -> Vec<String> {
    if let Some(ext) = Path::new(command).extension().and_then(|e| e.to_str()) {
        let dot_ext = format!(".{}", ext.to_ascii_uppercase());
        if exts.contains(&dot_ext) {
            let stem = &command[..command.len() - dot_ext.len()];
            return vec![
                command.to_owned(),
                format!("{stem}{dot_ext}"),
                format!("{stem}{}", dot_ext.to_ascii_lowercase()),
            ];
        }
    }
    let mut out = Vec::with_capacity(exts.len() * 2);
    for ext in exts {
        out.push(format!("{command}{ext}"));
        out.push(format!("{command}{}", ext.to_ascii_lowercase()));
    }
    out
}

fn normalize_path_entry(raw: &str) -> &str {
    let trimmed = raw.trim();
    if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
        trimmed[1..trimmed.len() - 1].trim()
    } else {
        trimmed
    }
}

fn is_executable_file(path: &Path, windows_exts: &[String]) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(windows)]
    {
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            return false;
        };
        let dot_ext = format!(".{}", ext.to_ascii_uppercase());
        windows_exts.contains(&dot_ext)
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = windows_exts;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = windows_exts;
        true
    }
}
