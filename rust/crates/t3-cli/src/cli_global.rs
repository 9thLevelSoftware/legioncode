//! Global CLI flags (`--log-level`, `--completions`, `--wizard`) and Effect CLI error formatting.

use crate::cmd_completions::run_completions;
use crate::cmd_help::{has_help_flag, print_help_for_args, print_help_for_path};
use crate::cmd_help_catalog::resolve_command_path;
use crate::fallback::write_stdout_raw;

const VALID_LOG_LEVELS: [&str; 9] = [
    "all", "trace", "debug", "info", "warn", "warning", "error", "fatal", "none",
];

const VALID_COMPLETION_SHELLS: [&str; 6] = ["bash", "zsh", "fish", "powershell", "sh", "pwsh"];

const VALUE_TAKING_FLAGS: &[&str] = &[
    "--mode",
    "--port",
    "--host",
    "--base-dir",
    "--dev-url",
    "--bootstrap-fd",
    "--tailscale-serve-port",
    "--ttl",
    "--label",
    "--subject",
    "--base-url",
    "--scopes",
    "--url",
    "--pairing-code",
    "--access",
    "--title",
    "--provider",
    "--model",
    "--option",
    "--worktree",
    "--branch",
    "--messages",
    "--project",
    "--view",
    "--limit",
    "--after",
    "--run",
    "--timeout",
    "--since",
    "--policy",
    "--id",
    "--database-path",
    "--launcher-protocol",
    "--log-file",
    "--service-unit",
    "--remote-url",
    "--token",
];

/// Outcome of global flag parsing before command-specific dispatch.
pub(crate) enum GlobalPrepass {
    /// Cleaned arguments with global `--log-level` / `--wizard` / `--completions` stripped.
    Continue { cleaned: Vec<String> },
    /// Pre-pass already completed execution or emitted an Effect CLI diagnostic.
    Exit(i32),
}

/// Validates and strips global `--log-level`, handles `--completions` and `--wizard`, and returns
/// cleaned arguments for downstream subcommand parsers.
pub(crate) fn run_global_prepass(raw_args: &[String]) -> GlobalPrepass {
    let mut cleaned = Vec::with_capacity(raw_args.len());
    let mut completions_shell: Option<String> = None;
    let mut has_wizard = false;
    let mut idx = 0;

    while idx < raw_args.len() {
        let arg = &raw_args[idx];
        if arg == "--" {
            cleaned.extend_from_slice(&raw_args[idx..]);
            break;
        }
        if VALUE_TAKING_FLAGS.contains(&arg.as_str()) {
            cleaned.push(arg.clone());
            if let Some(val) = raw_args.get(idx + 1) {
                cleaned.push(val.clone());
                idx += 2;
            } else {
                idx += 1;
            }
            continue;
        }
        if arg == "--log-level" || arg.starts_with("--log-level=") {
            match consume_choice_flag(raw_args, &mut idx, "--log-level", &VALID_LOG_LEVELS) {
                Ok(_level) => {
                    idx += 1;
                    continue;
                }
                Err(code) => return GlobalPrepass::Exit(code),
            }
        }
        if arg == "--completions" || arg.starts_with("--completions=") {
            match consume_choice_flag(
                raw_args,
                &mut idx,
                "--completions",
                &VALID_COMPLETION_SHELLS,
            ) {
                Ok(shell) => {
                    completions_shell = Some(shell);
                    idx += 1;
                    continue;
                }
                Err(code) => return GlobalPrepass::Exit(code),
            }
        }
        if arg == "--wizard" {
            has_wizard = true;
            idx += 1;
            continue;
        }
        cleaned.push(arg.clone());
        idx += 1;
    }

    if let Some(shell) = completions_shell {
        return GlobalPrepass::Exit(run_completions(&shell, raw_args));
    }
    if has_wizard && !has_help_flag(&cleaned) {
        return GlobalPrepass::Exit(run_wizard_cancel(&cleaned));
    }
    GlobalPrepass::Continue { cleaned }
}

/// Prints `path` help (if resolved) to stdout and Effect CLI `\nERROR\n  <message>` to stderr.
pub(crate) fn fail_with_command_help(path: &[&str], message: &str) -> i32 {
    print_help_for_path(path);
    eprintln!("\nERROR\n  {message}");
    1
}

/// Prints `\nERROR\n  <message>` to stderr without emitting stdout help, returning exit code `1`.
pub(crate) fn fail_cli_error(message: &str) -> i32 {
    eprintln!("\nERROR\n  {message}");
    1
}

fn consume_choice_flag(
    raw_args: &[String],
    idx: &mut usize,
    flag: &str,
    valid: &[&str],
) -> Result<String, i32> {
    let arg = &raw_args[*idx];
    let val = if let Some((_, inline)) = arg.split_once('=') {
        inline.to_owned()
    } else if let Some(next) = raw_args.get(*idx + 1).filter(|v| !v.starts_with('-')) {
        *idx += 1;
        next.clone()
    } else {
        print_help_for_args(raw_args);
        let expected = valid.join(" | ");
        eprintln!("\nERROR\n  Missing value for flag {flag}. Expected: {expected}");
        return Err(1);
    };

    if valid.contains(&val.as_str()) {
        Ok(val)
    } else {
        print_help_for_args(raw_args);
        let expected = valid
            .iter()
            .map(|item| format!("\"{item}\""))
            .collect::<Vec<_>>()
            .join(" | ");
        eprintln!("\nERROR\n  Invalid value for flag {flag}: \"{val}\". Expected: {expected}");
        Err(1)
    }
}

fn run_wizard_cancel(cleaned: &[String]) -> i32 {
    let path = resolve_command_path(cleaned);
    let cmd_display = if path.is_empty() {
        "t3".to_owned()
    } else {
        format!("t3 {}", path.join(" "))
    };
    let text = format!(
        "t3 v{} \u{00B7} Command wizard\n\
         Run T3 Code in this terminal and open it in your browser. To use it from your other devices, run `t3 up`.\n\
         Build a command interactively. Press Ctrl+C to cancel.\n\n\
         Current command\n  $ {cmd_display}\n\n\
         Wizard cancelled.\n",
        env!("CARGO_PKG_VERSION")
    );
    write_stdout_raw(&text, "wizard output")
}
