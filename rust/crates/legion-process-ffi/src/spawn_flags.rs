//! Windows `CREATE_NO_WINDOW` and cross-platform spawn-window rules (`d2bb611134` #38194).
//!
//! Ensures background child processes (dev servers, workers, git subprocesses, and
//! provider/editor probes) never flash a console window or detach into a visible
//! Windows console host on spawn.

use std::process::Command;

use serde::{Deserialize, Serialize};

/// Win32 process creation flag suppressing new console windows (`0x0800_0000`).
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// Win32 process creation flag creating a detached process (`0x0000_0008`).
pub const DETACHED_PROCESS: u32 = 0x0000_0008;
/// Win32 process creation flag starting a new process group (`0x0000_0200`).
pub const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

/// Target operating system driving spawn window/detachment rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetOs {
    Windows,
    MacOs,
    Linux,
    Other,
}

impl TargetOs {
    /// Returns the compile-time host target OS.
    pub const fn host() -> Self {
        if cfg!(target_os = "windows") {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(target_os = "linux") {
            Self::Linux
        } else {
            Self::Other
        }
    }

    /// Parses an OS identifier such as `"win32"`, `"windows"`, `"darwin"`, `"macos"`, or `"linux"`.
    pub fn from_os_name(os: &str) -> Self {
        match os.trim().to_ascii_lowercase().as_str() {
            "win32" | "windows" => Self::Windows,
            "darwin" | "macos" => Self::MacOs,
            "linux" => Self::Linux,
            _ => Self::Other,
        }
    }
}

/// Logical role of a spawned child process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChildProcessRole {
    /// Local dev runner (`vp run dev`, Vite, backend dev server).
    DevServer,
    /// Background helper, sidecar, or MCP/provider worker process.
    Worker,
    /// Short-lived git CLI invocation (`status`, `diff`, `fetch`, `ls-files`).
    GitSubprocess,
    /// Binary/editor/environment probe command (`--version`, `which`, `where`).
    ProbeSubprocess,
    /// Explicit interactive console process that intentionally owns a visible window.
    InteractiveConsole,
}

impl ChildProcessRole {
    /// Returns `true` when this role runs headlessly in the background without a console UI.
    pub const fn is_background(self) -> bool {
        !matches!(self, Self::InteractiveConsole)
    }
}

/// Resolved process-window and detachment policy for a child spawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpawnWindowPolicy {
    /// Win32 `dwCreationFlags` passed to `CreateProcessW` (zero on non-Windows).
    pub creation_flags: u32,
    /// Equivalent `windowsHide` setting for Node/Electron child-process callers.
    pub windows_hide: bool,
    /// Whether the child should detach into its own process group (`false` on Windows per #38194).
    pub detached: bool,
    /// Whether process-tree teardown uses kernel Job Objects / `taskkill /T` instead of PGID signals.
    pub teardown_via_tree_kill: bool,
}

/// Computes the exact [`SpawnWindowPolicy`] for `role` on `os` (`#38194`).
pub const fn policy_for(role: ChildProcessRole, os: TargetOs) -> SpawnWindowPolicy {
    match os {
        TargetOs::Windows => {
            if role.is_background() {
                // Windows tears down via Job Object / `taskkill /T`; a detached shell opens a
                // visible console window, so `detached` stays `false` and `CREATE_NO_WINDOW` is set.
                SpawnWindowPolicy {
                    creation_flags: CREATE_NO_WINDOW,
                    windows_hide: true,
                    detached: false,
                    teardown_via_tree_kill: true,
                }
            } else {
                SpawnWindowPolicy {
                    creation_flags: 0,
                    windows_hide: false,
                    detached: false,
                    teardown_via_tree_kill: true,
                }
            }
        }
        TargetOs::MacOs | TargetOs::Linux | TargetOs::Other => SpawnWindowPolicy {
            creation_flags: 0,
            windows_hide: false,
            detached: matches!(role, ChildProcessRole::DevServer),
            teardown_via_tree_kill: false,
        },
    }
}

/// Returns the Win32 `dwCreationFlags` for `role` on `os`.
pub const fn creation_flags_for(role: ChildProcessRole, os: TargetOs) -> u32 {
    policy_for(role, os).creation_flags
}

/// Returns whether a dev-server runner should detach into its own process group on `os` (`#38194`).
pub const fn should_detach_dev_server(os: TargetOs) -> bool {
    policy_for(ChildProcessRole::DevServer, os).detached
}

/// Applies `policy` onto a `std::process::Command` (sets `CREATE_NO_WINDOW` on Windows targets).
pub fn apply_windows_spawn_flags(command: &mut Command, policy: SpawnWindowPolicy) {
    #[cfg(windows)]
    if policy.creation_flags != 0 {
        use std::os::windows::process::CommandExt;
        command.creation_flags(policy.creation_flags);
    }
    #[cfg(not(windows))]
    let _ = (command, policy);
}
