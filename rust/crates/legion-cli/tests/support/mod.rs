//! Shared CLI process test runner helpers.

use std::error::Error;
use std::path::Path;
use std::process::{Command, Output};

pub type TestResult = Result<(), Box<dyn Error>>;

/// Runs the compiled `legioncode` binary with ambient `RUST_ANYTHING_*` and `LEGIONCODE_*` vars cleared
/// before applying `envs`.
pub fn run_legioncode(args: &[&str], envs: &[(&str, &str)]) -> Result<Output, std::io::Error> {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_legioncode"));
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
        "LEGIONCODE_BIN_TS",
        "LEGIONCODE_BOOT_SERVICE_UNIT_PATH",
        "LEGIONCODE_INSTALL_BIN_DIR",
        "LEGIONCODE_RELEASE_BASE_URL",
        "SSH_CONNECTION",
        "SSH_TTY",
        "LEGIONCODE_BOOT_SERVICE_UNIT",
        "LEGIONCODE_HOME",
        "LEGIONCODE_STATIC_DIR",
        "LEGIONCODE_DEV_AUTH_TOKEN",
        "LEGIONCODE_ACP_MCP_ENDPOINT",
        "LEGIONCODE_ACP_MCP_AUTHORIZATION",
        "LEGIONCODE_MCP_ENDPOINT",
        "LEGIONCODE_MCP_AUTHORIZATION",
        "LEGIONCODE_MANAGED_POLICY_FILE",
        "LEGIONCODE_PORT",
        "LEGIONCODE_HOST",
        "LEGIONCODE_TRACE_FILE",
        "LEGIONCODE_TRACE_MAX_FILES",
        "LEGIONCODE_CLOUD_PUBLIC_CONFIG",
        "LEGIONCODE_RELAY_URL",
        "LEGIONCODE_CLERK_PUBLISHABLE_KEY",
        "LEGIONCODE_CLERK_CLI_OAUTH_CLIENT_ID",
        "LEGIONCODE_CLI_PATH",
        "LEGIONCODE_NPM_LAUNCHER_PATH",
        "LEGIONCODE_SURFACE_APP_DIR",
        "LEGIONCODE_LEGACY_NODE",
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
