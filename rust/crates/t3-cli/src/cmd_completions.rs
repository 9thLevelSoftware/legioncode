//! Shell completion script emission (`--completions <bash|zsh|fish|powershell|sh|pwsh>`).

use crate::fallback::{delegate_to_node, unit_forced_original, write_stdout_raw};

/// Unit identifier for `RUST_ANYTHING_UNIT_COMPLETIONS` and fallback logs.
pub const UNIT_COMPLETIONS: &str = "COMPLETIONS";

const BASH_COMPLETION: &str = include_str!("../assets/completions/bash.sh");
const ZSH_COMPLETION: &str = include_str!("../assets/completions/zsh.zsh");
const FISH_COMPLETION: &str = include_str!("../assets/completions/fish.fish");
const POWERSHELL_COMPLETION: &str = include_str!("../assets/completions/powershell.ps1");

/// Prints the embedded completion script for `shell` unless the `COMPLETIONS` kill switch is set.
pub(crate) fn run_completions(shell: &str, raw_args: &[String]) -> i32 {
    if unit_forced_original(UNIT_COMPLETIONS) {
        return delegate_to_node(UNIT_COMPLETIONS, raw_args);
    }

    let Some(script) = script_for_shell(shell) else {
        eprintln!(
            "\nERROR\n  Invalid value for flag --completions: \"{shell}\". Expected: \"bash\" | \"zsh\" | \"fish\" | \"powershell\" | \"sh\" | \"pwsh\""
        );
        return 1;
    };

    write_stdout_raw(script, "shell completion script")
}

/// Maps a CLI shell name or alias (`sh` -> `bash`, `pwsh` -> `powershell`) to its static script.
pub(crate) fn script_for_shell(shell: &str) -> Option<&'static str> {
    match shell {
        "bash" | "sh" => Some(BASH_COMPLETION),
        "zsh" => Some(ZSH_COMPLETION),
        "fish" => Some(FISH_COMPLETION),
        "powershell" | "pwsh" => Some(POWERSHELL_COMPLETION),
        _ => None,
    }
}
