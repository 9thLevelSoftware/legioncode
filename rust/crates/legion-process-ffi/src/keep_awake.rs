//! Cross-platform keep-awake inhibitor command specification (`414a068f67` #39490).
//!
//! Mirrors `apps/server/src/background/KeepAwake.ts` while enforcing `#39490`: on Linux,
//! `systemd-inhibit` requests `--what=sleep` and NEVER `--what=idle:sleep`, so desktop
//! screen locking and display power-off continue working normally while agents run.

use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::spawn_flags::{ChildProcessRole, TargetOs, apply_windows_spawn_flags, policy_for};

/// Required Linux `systemd-inhibit --what` flag allowing screen lock and display power-off.
pub const SYSTEMD_INHIBIT_WHAT_SLEEP: &str = "--what=sleep";
/// Forbidden `--what=idle:sleep` flag that blocked screen lock and DPMS display-off prior to #39490.
pub const FORBIDDEN_SYSTEMD_INHIBIT_WHAT_IDLE_SLEEP: &str = "--what=idle:sleep";
/// Default `--who` inhibitor owner name.
pub const DEFAULT_INHIBITOR_WHO: &str = "LegionCode";
/// Default `--why` inhibitor reason.
pub const DEFAULT_INHIBITOR_WHY: &str = "Agents are working";

const WINDOWS_HOLD_SCRIPT: &str = concat!(
    "$t = Add-Type -Name Power -Namespace Legion -PassThru -MemberDefinition ",
    "'[DllImport(\"kernel32.dll\")] public static extern uint SetThreadExecutionState(uint f);'; ",
    "[void]$t::SetThreadExecutionState([uint32]2147483649); ",
    "[Console]::Out.WriteLine()"
);

/// Target platform environment selecting the host sleep-inhibitor command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeepAwakePlatform {
    Linux,
    LinuxWsl,
    MacOs,
    Windows,
    Other,
}

impl KeepAwakePlatform {
    /// Maps a [`TargetOs`] and WSL kernel flag to a [`KeepAwakePlatform`].
    pub const fn from_os(os: TargetOs, is_wsl: bool) -> Self {
        match os {
            TargetOs::Linux if is_wsl => Self::LinuxWsl,
            TargetOs::Linux => Self::Linux,
            TargetOs::MacOs => Self::MacOs,
            TargetOs::Windows => Self::Windows,
            TargetOs::Other => Self::Other,
        }
    }
}

/// Stdin wiring required by the inhibitor child process (`KeepAwake.ts:38`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InhibitorStdin {
    /// Redirect stdin to null (`/dev/null` or `NUL`).
    Ignore,
    /// Keep a parent-owned pipe open so EOF releases the Windows inhibitor when WSL exits.
    Pipe,
}

/// Errors produced when validating parameters for a keep-awake command specification.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum KeepAwakeError {
    #[error("keep-awake server PID must be positive")]
    InvalidServerPid,
    #[error("keep-awake inhibitor `{field}` must not be empty or contain NUL/newlines")]
    InvalidMetadata { field: &'static str },
}

/// Validated sleep-inhibitor child process specification (`KeepAwake.ts:30-92`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeepAwakeSpec {
    /// Executable program name (`"systemd-inhibit"`, `"sh"`, or `"powershell.exe"`).
    pub program: String,
    /// Literal arguments passed with `shell: false`.
    pub args: Vec<String>,
    /// Required stdin wiring (`Pipe` under WSL, `Ignore` elsewhere).
    pub stdin: InhibitorStdin,
}

impl KeepAwakeSpec {
    /// Converts this specification into a `std::process::Command` with `CREATE_NO_WINDOW` on Windows.
    pub fn to_std_command(&self) -> Command {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.args);
        cmd.stdin(match self.stdin {
            InhibitorStdin::Ignore => Stdio::null(),
            InhibitorStdin::Pipe => Stdio::piped(),
        });
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        apply_windows_spawn_flags(
            &mut cmd,
            policy_for(ChildProcessRole::Worker, TargetOs::host()),
        );
        cmd
    }

    /// Returns `true` when this spec invokes Linux `systemd-inhibit` with `--what=sleep` and
    /// without `--what=idle:sleep`.
    pub fn uses_linux_sleep_only_inhibit(&self) -> bool {
        self.program == "systemd-inhibit"
            && self
                .args
                .iter()
                .any(|arg| arg == SYSTEMD_INHIBIT_WHAT_SLEEP)
            && !self
                .args
                .iter()
                .any(|arg| arg == FORBIDDEN_SYSTEMD_INHIBIT_WHAT_IDLE_SLEEP)
    }
}

/// Returns `true` when `/proc/sys/kernel/osrelease` identifies a Microsoft WSL kernel.
pub fn is_wsl_kernel_release(osrelease: &str) -> bool {
    osrelease.to_ascii_lowercase().contains("microsoft")
}

/// Builds the platform keep-awake inhibitor spec using default `"LegionCode"` / `"Agents are working"`
/// metadata, returning `Ok(None)` when the platform has no inhibitor (`Other`).
pub fn build_default_keep_awake_spec(
    platform: KeepAwakePlatform,
    server_pid: u32,
) -> Result<Option<KeepAwakeSpec>, KeepAwakeError> {
    build_keep_awake_spec(
        platform,
        server_pid,
        DEFAULT_INHIBITOR_WHO,
        DEFAULT_INHIBITOR_WHY,
    )
}

/// Builds a validated sleep-inhibitor command spec for `platform` watching `server_pid`.
pub fn build_keep_awake_spec(
    platform: KeepAwakePlatform,
    server_pid: u32,
    who: &str,
    why: &str,
) -> Result<Option<KeepAwakeSpec>, KeepAwakeError> {
    if server_pid == 0 {
        return Err(KeepAwakeError::InvalidServerPid);
    }
    validate_metadata("who", who)?;
    validate_metadata("why", why)?;

    let spec = match platform {
        KeepAwakePlatform::LinuxWsl => KeepAwakeSpec {
            program: "powershell.exe".to_owned(),
            args: vec![
                "-NoProfile".to_owned(),
                "-NonInteractive".to_owned(),
                "-Command".to_owned(),
                format!("{WINDOWS_HOLD_SCRIPT}; [void][Console]::In.ReadToEnd()"),
            ],
            stdin: InhibitorStdin::Pipe,
        },
        KeepAwakePlatform::MacOs => KeepAwakeSpec {
            program: "sh".to_owned(),
            args: vec![
                "-c".to_owned(),
                "echo && exec caffeinate -i -w \"$0\"".to_owned(),
                server_pid.to_string(),
            ],
            stdin: InhibitorStdin::Ignore,
        },
        KeepAwakePlatform::Linux => build_linux_systemd_inhibit_spec(server_pid, who, why),
        KeepAwakePlatform::Windows => KeepAwakeSpec {
            program: "powershell.exe".to_owned(),
            args: vec![
                "-NoProfile".to_owned(),
                "-NonInteractive".to_owned(),
                "-Command".to_owned(),
                format!("{WINDOWS_HOLD_SCRIPT}; Wait-Process -Id {server_pid}"),
            ],
            stdin: InhibitorStdin::Ignore,
        },
        KeepAwakePlatform::Other => return Ok(None),
    };
    Ok(Some(spec))
}

fn build_linux_systemd_inhibit_spec(server_pid: u32, who: &str, why: &str) -> KeepAwakeSpec {
    KeepAwakeSpec {
        program: "systemd-inhibit".to_owned(),
        args: vec![
            SYSTEMD_INHIBIT_WHAT_SLEEP.to_owned(),
            format!("--who={}", who.trim()),
            format!("--why={}", why.trim()),
            "--mode=block".to_owned(),
            "sh".to_owned(),
            "-c".to_owned(),
            "echo && exec tail --pid=\"$0\" -f /dev/null".to_owned(),
            server_pid.to_string(),
        ],
        stdin: InhibitorStdin::Ignore,
    }
}

fn validate_metadata(field: &'static str, value: &str) -> Result<(), KeepAwakeError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.contains('\0') || trimmed.contains(['\r', '\n']) {
        return Err(KeepAwakeError::InvalidMetadata { field });
    }
    Ok(())
}
