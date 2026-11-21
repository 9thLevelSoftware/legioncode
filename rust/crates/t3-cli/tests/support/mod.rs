//! Shared CLI process test runner helpers.

use std::error::Error;
use std::path::Path;
use std::process::{Command, Output};

pub type TestResult = Result<(), Box<dyn Error>>;

/// Runs the compiled `t3` binary with ambient `RUST_ANYTHING_*` and `T3CODE_*` vars cleared
/// before applying `envs`.
pub fn run_t3(args: &[&str], envs: &[(&str, &str)]) -> Result<Output, std::io::Error> {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_t3"));
    cmd.args(args);
    for key in [
        "RUST_ANYTHING_FALLBACK_LOG",
        "RUST_ANYTHING_UNIT_VERSION",
        "RUST_ANYTHING_UNIT_STATUS",
        "RUST_ANYTHING_UNIT_POLICY",
        "RUST_ANYTHING_UNIT_DOCTOR",
        "RUST_ANYTHING_UNIT_SERVE",
        "RUST_ANYTHING_UNIT_SERVICE_PREFLIGHT",
        "RUST_ANYTHING_UNIT_PROJECT",
        "RUST_ANYTHING_UNIT_SERVICE",
        "RUST_ANYTHING_UNIT_SPEECH_WORKER",
        "RUST_ANYTHING_UNIT_TRACE",
        "RUST_ANYTHING_UNIT_THEME",
        "RUST_ANYTHING_UNIT_ENVIRONMENT",
        "RUST_ANYTHING_UNIT_AUTH",
        "RUST_ANYTHING_UNIT_PAIR",
        "RUST_ANYTHING_UNIT_SSH_HELPER",
        "RUST_ANYTHING_UNIT_ACP_MCP",
        "RUST_ANYTHING_UNIT_THREAD",
        "RUST_ANYTHING_UNIT_SERVICE_LAUNCHER",
        "RUST_ANYTHING_UNIT_CONNECT",
        "RUST_ANYTHING_UNIT_CLI_PASSTHROUGH",
        "RUST_ANYTHING_UNIT_HELP",
        "RUST_ANYTHING_UNIT_UP",
        "RUST_ANYTHING_UNIT_TRIAGE",
        "RUST_ANYTHING_UNIT_APP",
        "RUST_ANYTHING_UNIT_BROWSER",
        "RUST_ANYTHING_UNIT_INSTALL",
        "RUST_ANYTHING_UNIT_UNINSTALL",
        "RUST_ANYTHING_UNIT_UPDATE",
        "RUST_ANYTHING_UNIT_DOCUMENT_TEXT",
        "RUST_ANYTHING_UNIT_MANAGED_TOKEN",
        "RUST_ANYTHING_UNIT_CLAUDE_HISTORY",
        "T3_BIN_TS",
        "T3CODE_BOOT_SERVICE_UNIT_PATH",
        "T3CODE_INSTALL_BIN_DIR",
        "T3CODE_RELEASE_BASE_URL",
        "SSH_CONNECTION",
        "SSH_TTY",
        "T3_BOOT_SERVICE_UNIT",
        "T3CODE_HOME",
        "T3CODE_STATIC_DIR",
        "T3CODE_DEV_AUTH_TOKEN",
        "T3_ACP_MCP_ENDPOINT",
        "T3_ACP_MCP_AUTHORIZATION",
        "T3CODE_MCP_ENDPOINT",
        "T3CODE_MCP_AUTHORIZATION",
        "T3CODE_MANAGED_POLICY_FILE",
        "T3CODE_PORT",
        "T3CODE_HOST",
        "T3CODE_TRACE_FILE",
        "T3CODE_TRACE_MAX_FILES",
        "T3CODE_CLOUD_PUBLIC_CONFIG",
        "T3CODE_RELAY_URL",
        "T3CODE_CLERK_PUBLISHABLE_KEY",
        "T3CODE_CLERK_CLI_OAUTH_CLIENT_ID",
        "T3CODE_CLI_PATH",
        "T3CODE_NPM_LAUNCHER_PATH",
        "T3CODE_SURFACE_APP_DIR",
        "T3CODE_LEGACY_NODE",
    ] {
        cmd.env_remove(key);
    }
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.output()
}

pub fn isolated_env<'a>(
    home_dir: &'a Path,
    extra: &[(&'a str, &'a str)],
) -> Vec<(&'a str, &'a str)> {
    let home_str = home_dir.to_str().unwrap_or(".");
    let mut vars = vec![
        ("LOCALAPPDATA", home_str),
        ("USERPROFILE", home_str),
        ("HOME", home_str),
        ("XDG_CONFIG_HOME", home_str),
    ];
    vars.extend_from_slice(extra);
    vars
}
