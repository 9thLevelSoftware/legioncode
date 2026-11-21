//! Triage prompt, context markdown, and composer file-link builders matching
//! `apps/server/src/triage/triagePrompt.ts` and `packages/shared/src/{composerTrigger,triagePrompt}.ts`.

use std::fmt::Write as _;

pub use crate::triage_prompt_playbook::TRIAGE_PLAYBOOK;

/// File name of the playbook prompt written inside each triage session folder.
pub const TRIAGE_PROMPT_FILE_NAME: &str = "triage-playbook.md";

/// File name of the boot service log under `logs_dir`.
pub const BOOT_SERVICE_LOG_FILE: &str = "boot-service.log";

/// File name of the T3 Connect tunnel client log under `logs_dir`.
pub const RELAY_CLIENT_LOG_FILE: &str = "relay-client.ndjson";

const IN_APP_SECTION: &str = r"
## This session runs inside T3 Code

You are a thread inside the T3 Code server described above. Stopping,
restarting, or killing that server, its service, or the desktop app ends this
session. When a fix needs one of those, give the user the exact steps and let
them do it. The database is in use by that server: read it, never lock it.
";

/// Origin of the triage session (`t3 triage` CLI vs in-app "Triage an issue").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TriageEntry {
    /// Started from the `t3 triage` command-line entrypoint.
    Cli {
        /// Installed CLI version string.
        cli_version: String,
        /// Command string used to invoke `t3`.
        launched_as: String,
    },
    /// Started from a UI client.
    App {
        /// Client surface label (`"desktop"`, `"web"`, etc.).
        client: String,
    },
}

/// Optional source checkout clone status included in `context.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TriageSourceClone {
    /// Clone completed cleanly.
    Cloned {
        /// Destination checkout directory.
        directory: String,
        /// Git ref checked out.
        git_ref: String,
        /// Checked-out commit SHA.
        commit: String,
        /// Recorded server version.
        server_version: String,
    },
    /// Clone is currently running in background.
    Cloning {
        /// Destination checkout directory.
        directory: String,
        /// Git ref being checked out.
        git_ref: String,
        /// Expected commit SHA.
        commit: String,
        /// Recorded server version.
        server_version: String,
    },
    /// Clone failed before completion.
    Failed {
        /// Recorded server version.
        server_version: String,
        /// Failure diagnostic message.
        reason: String,
    },
}

/// Path summary fields rendered in the `## Paths` section of `context.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriagePaths {
    pub state_dir: String,
    pub db_path: String,
    pub settings_path: String,
    pub logs_dir: String,
    pub service_log_path: String,
    pub desktop_backend_log_glob: String,
    pub relay_client_log_path: String,
    pub server_trace_path: String,
    pub provider_event_log_path: String,
    pub terminal_logs_dir: String,
    pub provider_status_cache_dir: String,
    pub secrets_dir: String,
    pub source_cache_dir: String,
}

/// Complete input used to render `context.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriageContextInput {
    pub generated_at: String,
    pub entry: TriageEntry,
    pub os: String,
    pub runtime_version: String,
    pub server: String,
    pub source_clone: Option<TriageSourceClone>,
    pub paths: TriagePaths,
}

/// Windows `.cmd`-safe single-line launch prompt pointing to `prompt_file_path`
/// (`triagePrompt.ts:175-176`).
///
/// # Examples
///
/// ```
/// use t3_persistence::triage_session::build_triage_launch_prompt;
///
/// assert!(build_triage_launch_prompt("/tmp/triage-playbook.md").contains("/tmp/triage-playbook.md"));
/// ```
pub fn build_triage_launch_prompt(prompt_file_path: &str) -> String {
    format!(
        "Read the file \"{prompt_file_path}\" and follow its instructions exactly: it is your T3 Code triage playbook, and it starts with asking the user what went wrong."
    )
}

/// Formats `[escaped-basename](encoded-path)` matching `serializeComposerFileLink` in
/// `packages/shared/src/composerTrigger.ts:31-34`.
pub fn serialize_composer_file_link(path: &str) -> String {
    let base = match (path.rfind('/'), path.rfind('\\')) {
        (Some(a), Some(b)) => &path[a.max(b) + 1..],
        (Some(a), None) => &path[a + 1..],
        (None, Some(b)) => &path[b + 1..],
        (None, None) => path,
    };
    let mut label = String::with_capacity(base.len());
    for ch in base.chars() {
        if matches!(ch, '\\' | '[' | ']' | '*' | '_') {
            label.push('\\');
        }
        label.push(ch);
    }
    let encoded = encode_uri_link_dest(path);
    format!("[{label}]({encoded})")
}

/// Composer prompt prefix opened for in-app triage sessions (`triagePrompt.ts:183-184`).
pub fn build_triage_app_prompt(prompt_file_path: &str) -> String {
    format!(
        "Read {} and follow it. ",
        serialize_composer_file_link(prompt_file_path)
    )
}

/// Builds the full seed playbook file content (`triage-playbook.md`, `triagePrompt.ts:186-205`).
pub fn build_triage_seed_prompt(context_file_path: &str, entry: &TriageEntry) -> String {
    let started = match entry {
        TriageEntry::Cli { .. } => "with `t3 triage`",
        TriageEntry::App { .. } => "from the T3 Code app",
    };
    let follow = match entry {
        TriageEntry::Cli { .. } => {
            "Follow the playbook below, starting by asking the user what went wrong."
        }
        TriageEntry::App { .. } => {
            "Follow the playbook below. The message that sent you here may already describe what went wrong; if it does not, start by asking."
        }
    };
    format!(
        "A T3 Code user is having a problem with their install and started this session {started}.\n\n\
         Machine facts (versions, OS, paths, server liveness) are in the triage context file:\n\n    \
         {context_file_path}\n\n\
         {follow}\n\n\
         ---\n\n\
         {TRIAGE_PLAYBOOK}"
    )
}

/// Renders the `context.md` document (`triagePrompt.ts:297-325`).
pub fn build_triage_context(input: &TriageContextInput) -> String {
    let (generated_by, before, after) = match &input.entry {
        TriageEntry::Cli {
            cli_version,
            launched_as,
        } => (
            "`t3 triage`",
            format!(
                "- Triage CLI version: {cli_version} (the `t3` that ran this command; the bug may have happened on another build)"
            ),
            Some(format!("- CLI launched as: {launched_as}")),
        ),
        TriageEntry::App { client } => (
            "the T3 Code app",
            format!(
                "- Started from: {client} (the client the user pressed \"Triage an issue\" in; the bug may have happened on another device)"
            ),
            None,
        ),
    };

    let after_line = after.map_or_else(String::new, |line| format!("{line}\n"));
    let in_app = if matches!(input.entry, TriageEntry::App { .. }) {
        IN_APP_SECTION
    } else {
        ""
    };
    let source = input
        .source_clone
        .as_ref()
        .map_or_else(String::new, format_source_section);
    let p = &input.paths;

    format!(
        "# T3 Code triage context\n\n\
         Generated by {generated_by} at {generated_at}.\n\n\
         {before}\n\
         - OS: {os}\n\
         - Runtime: {runtime}\n\
         {after_line}- Server process: {server}\n\
         - Repo: https://github.com/pingdotgg/t3code\n\
         {in_app}{source}\n\
         ## Paths\n\n\
         - State dir: {state_dir}\n\
         - Database (SQLite; write only with the user's explicit permission): {db_path}\n\
         - Settings: {settings_path}\n\
         - Logs dir: {logs_dir}\n\
         - Service log (systemd/launchd service installs only): {service_log_path}\n\
         - Desktop backend logs (glob; one file per backend plus rotated copies; written only when a backend crashes or fails to start): {desktop_backend_log_glob}\n\
         - T3 Connect tunnel log (ndjson; what the server logged about its cloudflared connector, with rotated copies `.1` and `.2`): {relay_client_log_path}\n\
         - Server trace (ndjson): {server_trace_path}\n\
         - Provider event log: {provider_event_log_path}\n\
         - Terminal logs: {terminal_logs_dir}\n\
         - Provider status cache: {provider_status_cache_dir}\n\
         - Secrets dir (NEVER read this): {secrets_dir}\n\
         - Source cache dir (clone the repo here): {source_cache_dir}\n",
        generated_at = input.generated_at,
        os = input.os,
        runtime = input.runtime_version,
        server = input.server,
        state_dir = p.state_dir,
        db_path = p.db_path,
        settings_path = p.settings_path,
        logs_dir = p.logs_dir,
        service_log_path = p.service_log_path,
        desktop_backend_log_glob = p.desktop_backend_log_glob,
        relay_client_log_path = p.relay_client_log_path,
        server_trace_path = p.server_trace_path,
        provider_event_log_path = p.provider_event_log_path,
        terminal_logs_dir = p.terminal_logs_dir,
        provider_status_cache_dir = p.provider_status_cache_dir,
        secrets_dir = p.secrets_dir,
        source_cache_dir = p.source_cache_dir,
    )
}

fn format_source_section(clone: &TriageSourceClone) -> String {
    match clone {
        TriageSourceClone::Failed {
            server_version,
            reason,
        } => format!(
            "\n## Source\n\n\
             The server could not clone the source for its version ({server_version}): {reason}\n\
             Clone it as step 4 describes.\n"
        ),
        TriageSourceClone::Cloned {
            directory,
            git_ref,
            commit,
            server_version,
        } => format_cloned_or_cloning(true, directory, git_ref, commit, server_version),
        TriageSourceClone::Cloning {
            directory,
            git_ref,
            commit,
            server_version,
        } => format_cloned_or_cloning(false, directory, git_ref, commit, server_version),
    }
}

fn format_cloned_or_cloning(
    finished: bool,
    directory: &str,
    git_ref: &str,
    commit: &str,
    server_version: &str,
) -> String {
    let verb = if finished {
        "The server cloned"
    } else {
        "The server is cloning"
    };
    let what = if git_ref == "main" {
        format!("main, since its version ({server_version}) has no release tag")
    } else {
        format!("the source for its own version ({server_version})")
    };
    let wait = if finished {
        ""
    } else {
        "\nThe folder appears only when the clone is complete. Wait for it instead of\ncloning again. If it has not appeared after a few minutes, clone as step 4\ndescribes.\n"
    };
    format!(
        "\n## Source\n\n{verb} {what}:\n\n    {directory}  ({git_ref}, commit {commit})\n{wait}\n\
         Use it when the bug happened on this server's build. When it happened on another\n\
         build, clone that version as step 4 describes.\n"
    )
}

fn encode_uri_link_dest(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for byte in path.as_bytes() {
        let keep = byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'-' | b'_'
                    | b'.'
                    | b'!'
                    | b'~'
                    | b'*'
                    | b'\''
                    | b';'
                    | b','
                    | b'/'
                    | b':'
                    | b'@'
                    | b'&'
                    | b'='
                    | b'+'
                    | b'$'
            );
        if keep {
            out.push(char::from(*byte));
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}
