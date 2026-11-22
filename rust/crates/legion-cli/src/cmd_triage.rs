//! `legioncode triage` problem investigation session creator and agent launcher (`cli/triage.ts`).

use std::fmt::Write as _;
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use legion_persistence::paths::ServerDerivedPaths;
use legion_persistence::triage_session::{
    TriageEntry, WriteTriageSessionInput, build_triage_launch_prompt, describe_server_process,
    write_triage_session,
};

use crate::cli_global::fail_with_command_help;
use crate::fallback::{
    delegate_to_node, resolve_base_dir, take_flag_value, unit_forced_original, user_home_dir,
};
use crate::invocation::{non_empty_env, resolve_command_on_path, resolve_host_cli_command};

/// Unit identifier for `RUST_ANYTHING_UNIT_TRIAGE` and fallback logs.
pub const UNIT_TRIAGE: &str = "TRIAGE";

#[derive(Clone, Copy)]
struct TriageAgent {
    id: &'static str,
    command: &'static str,
    label: &'static str,
    state_dir_env: &'static str,
    default_state_dir: &'static str,
    prompt_interactive: bool,
}

const TRIAGE_AGENTS: &[TriageAgent] = &[
    TriageAgent {
        id: "claude",
        command: "claude",
        label: "Claude Code",
        state_dir_env: "CLAUDE_CONFIG_DIR",
        default_state_dir: ".claude",
        prompt_interactive: false,
    },
    TriageAgent {
        id: "codex",
        command: "codex",
        label: "Codex",
        state_dir_env: "CODEX_HOME",
        default_state_dir: ".codex",
        prompt_interactive: false,
    },
    TriageAgent {
        id: "antigravity",
        command: "agy",
        label: "Antigravity",
        state_dir_env: "GEMINI_HOME",
        default_state_dir: ".gemini",
        prompt_interactive: true,
    },
];

struct TriageFlags {
    base_dir: Option<String>,
    agent: Option<String>,
    model: Option<String>,
}

/// Runs `legioncode triage [--base-dir <dir>] [--agent claude|codex|antigravity] [--model <m>]` natively.
pub fn run(args: &[String]) -> i32 {
    if unit_forced_original(UNIT_TRIAGE) {
        return delegate_to_node(UNIT_TRIAGE, args);
    }

    let Some(flags) = parse_triage_flags(args.get(1..).unwrap_or(&[])) else {
        return fail_with_command_help(
            &["triage"],
            "Invalid or unrecognized arguments for `legioncode triage`.",
        );
    };

    if let Some(ref agent_id) = flags.agent
        && !TRIAGE_AGENTS
            .iter()
            .any(|candidate| candidate.id == agent_id)
    {
        eprintln!(
            "\nERROR\n  Invalid value for flag --agent: \"{agent_id}\". Expected: \"claude\" | \"codex\" | \"antigravity\""
        );
        return 1;
    }

    let (base_dir, _) = resolve_base_dir(flags.base_dir.as_deref());
    // Triage always records user-facing `userdata` paths even without `--base-dir`.
    let paths = ServerDerivedPaths::derive(&base_dir, false, true);
    let server = describe_server_process(&paths.server_runtime_state_path)
        .unwrap_or_else(|err| format!("unknown ({err})"));

    let session = match write_triage_session(&WriteTriageSessionInput {
        base_dir,
        paths,
        entry: TriageEntry::Cli {
            cli_version: env!("CARGO_PKG_VERSION").to_owned(),
            launched_as: resolve_host_cli_command("triage", &[]),
        },
        server,
        source_clone: None,
        now_ms: None,
        os_label: None,
        runtime_version: None,
    }) {
        Ok(created) => created,
        Err(err) => {
            eprintln!("Failed to create triage session: {err}");
            return 1;
        }
    };

    let installed: Vec<(TriageAgent, PathBuf)> = TRIAGE_AGENTS
        .iter()
        .filter_map(|&agent| {
            resolve_command_on_path(agent.command).map(|resolved| (agent, resolved))
        })
        .collect();

    let selected = match resolve_selected_agent(flags.agent.as_deref(), &installed) {
        Ok(choice) => choice,
        Err(code) => return code,
    };

    let Some((agent, resolved_cmd)) = selected else {
        println!(
            "No supported agent CLI (claude, codex, agy) was found on this machine.\n\n\
             The triage prompt and machine context were written to:\n  \
             {}\n  {}\n\n\
             Paste the prompt file into any coding agent to run triage by hand.",
            session.prompt_file_path.display(),
            session.context_file_path.display()
        );
        return 0;
    };

    launch_triage_agent(
        agent,
        &resolved_cmd,
        flags.model.as_deref(),
        &session.session_dir,
        &session.prompt_file_path,
        &session.context_file_path,
    )
}

fn parse_triage_flags(rest: &[String]) -> Option<TriageFlags> {
    let mut base_dir = None;
    let mut agent = None;
    let mut model = None;
    let mut idx = 0;

    while idx < rest.len() {
        if let Ok(Some(val)) = take_flag_value(rest, &mut idx, "base-dir") {
            base_dir = Some(val);
        } else if let Ok(Some(val)) = take_flag_value(rest, &mut idx, "agent") {
            agent = Some(val);
        } else if let Ok(Some(val)) = take_flag_value(rest, &mut idx, "model") {
            model = Some(val);
        } else {
            return None;
        }
        idx += 1;
    }

    Some(TriageFlags {
        base_dir,
        agent,
        model,
    })
}

fn resolve_selected_agent(
    requested: Option<&str>,
    installed: &[(TriageAgent, PathBuf)],
) -> Result<Option<(TriageAgent, PathBuf)>, i32> {
    if let Some(req) = requested {
        if let Some(found) = installed.iter().find(|(agent, _)| agent.id == req) {
            return Ok(Some(found.clone()));
        }
        let cmd = TRIAGE_AGENTS
            .iter()
            .find(|agent| agent.id == req)
            .map_or(req, |agent| agent.command);
        eprintln!(
            "TriageAgentUnavailableError: `{cmd}` is not installed or was not found on PATH."
        );
        return Err(1);
    }

    match installed.len() {
        0 => Ok(None),
        1 => Ok(Some(installed[0].clone())),
        _ => {
            if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
                eprintln!(
                    "TriageAgentChoiceRequiredError: Multiple supported agent CLIs are installed and there is no terminal to ask which to use. Re-run with --agent claude, --agent codex, or --agent antigravity."
                );
                return Err(1);
            }
            pick_agent_interactively(installed).map(Some).ok_or(1)
        }
    }
}

fn pick_agent_interactively(
    installed: &[(TriageAgent, PathBuf)],
) -> Option<(TriageAgent, PathBuf)> {
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let mut menu = String::from("Run triage with:\n");
    for (idx, (agent, _)) in installed.iter().enumerate() {
        let _ = writeln!(menu, "  [{}] {}", idx + 1, agent.label);
    }

    loop {
        print!("{menu}> ");
        let _ = std::io::stdout().flush();
        let line = lines.next()?.ok()?;
        let trimmed = line.trim();
        if let Ok(num) = trimmed.parse::<usize>()
            && num >= 1
            && let Some(item) = installed.get(num - 1)
        {
            return Some(item.clone());
        }
        if let Some(item) = installed
            .iter()
            .find(|(agent, _)| agent.id.eq_ignore_ascii_case(trimmed))
        {
            return Some(item.clone());
        }
    }
}

fn launch_triage_agent(
    agent: TriageAgent,
    resolved_cmd: &Path,
    model: Option<&str>,
    session_dir: &Path,
    prompt_file_path: &Path,
    context_file_path: &Path,
) -> i32 {
    let prompt_str = prompt_file_path.to_string_lossy();
    let launch_prompt = build_triage_launch_prompt(&prompt_str);
    let mut cmd_args: Vec<String> = Vec::new();
    if let Some(m) = model {
        cmd_args.push("--model".to_owned());
        cmd_args.push(m.to_owned());
    }
    if agent.prompt_interactive {
        cmd_args.push("--prompt-interactive".to_owned());
    }
    cmd_args.push(launch_prompt.clone());

    println!("Starting {}. It will ask what went wrong.\n", agent.label);
    let status = Command::new(resolved_cmd)
        .args(&cmd_args)
        .current_dir(session_dir)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status();

    match status {
        Ok(exit) if exit.success() => 0,
        Ok(exit) => {
            let code = exit.code().unwrap_or(1);
            let state_dir = non_empty_env(agent.state_dir_env).map_or_else(
                || user_home_dir().join(agent.default_state_dir),
                PathBuf::from,
            );
            println!(
                "\n{label} exited with code {code}.\n\n\
                 {label} keeps its state in {state_dir}.\n\
                 If that drive is out of space, free some up or set {env_name} to a folder on another drive.\n\n\
                 The triage prompt and machine context were written to:\n  \
                 {prompt}\n  {context}\n\n\
                 To retry, start `{cmd}` yourself with any flags it suggested, or use any coding agent, and give it this prompt:\n{launch_prompt}",
                label = agent.label,
                state_dir = state_dir.display(),
                env_name = agent.state_dir_env,
                prompt = prompt_file_path.display(),
                context = context_file_path.display(),
                cmd = agent.command,
            );
            code
        }
        Err(err) => {
            eprintln!(
                "TriageAgentSpawnError: Could not start `{}`: {err}",
                agent.command
            );
            1
        }
    }
}
