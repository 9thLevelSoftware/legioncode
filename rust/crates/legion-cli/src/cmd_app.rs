//! `legioncode app [<cwd>] [--base-dir <dir>]` desktop workspace activation client (`cli/app.ts`).

use std::env;
use std::path::Path;
use std::time::Duration;

use legion_shared::desktop_app_control::{
    DesktopControlPlatform, resolve_desktop_app_control_address,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::cli_global::fail_with_command_help;
use crate::fallback::{
    delegate_to_node, expand_home, resolve_base_dir, resolve_platform_path, take_flag_value,
    unit_forced_original, write_stdout_line,
};
use crate::invocation::non_empty_env;
use crate::pipe_ipc::{PipeExchangeError, current_unix_uid, exchange_json_line};

/// Unit identifier for `RUST_ANYTHING_UNIT_APP` and fallback logs.
pub const UNIT_APP: &str = "APP";

/// Protocol version matching `DESKTOP_APP_ACTIVATION_PROTOCOL_VERSION`.
pub const DESKTOP_APP_ACTIVATION_PROTOCOL_VERSION: u32 = 1;

const MAX_REQUEST_BYTES: usize = 16 * 1024;
const MAX_RESPONSE_BYTES: usize = 64 * 1024;
const IPC_TIMEOUT: Duration = Duration::from_secs(5);

struct AppFlags {
    base_dir: Option<String>,
    workspace: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DesktopActivationRequest<'a> {
    version: u32,
    request_id: &'a str,
    #[serde(rename = "type")]
    request_type: &'static str,
    workspace_root: &'a str,
    platform: &'static str,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DesktopActivationResponse {
    #[serde(default)]
    version: Option<u32>,
    request_id: String,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    ok: Option<bool>,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

/// Runs `legioncode app [<path>] [--base-dir <dir>]` natively unless `RUST_ANYTHING_UNIT_APP=original`.
pub fn run(args: &[String]) -> i32 {
    if unit_forced_original(UNIT_APP) {
        return delegate_to_node(UNIT_APP, args);
    }

    let Some(flags) = parse_app_flags(args.get(1..).unwrap_or(&[])) else {
        return fail_with_command_help(
            &["app"],
            "Invalid or unrecognized arguments for `legioncode app`.",
        );
    };

    if non_empty_env("SSH_CONNECTION").is_some() || non_empty_env("SSH_TTY").is_some() {
        eprintln!(
            "DesktopAppSshUnsupportedError: `legioncode app` only controls a desktop app on the same machine. It cannot run over SSH."
        );
        return 1;
    }

    let (base_dir, explicit) = resolve_base_dir(flags.base_dir.as_deref());
    let workspace_root = resolve_workspace_root(flags.workspace.as_deref());
    let mut state_dirs = vec![base_dir.join("userdata")];
    if !explicit {
        state_dirs.push(base_dir.join("dev"));
    }

    let platform = DesktopControlPlatform::current();
    let temp_dir = env::temp_dir().to_string_lossy().into_owned();
    let uid = current_unix_uid();
    let request_id = Uuid::new_v4().to_string();

    let Ok(payload) = serde_json::to_string(&DesktopActivationRequest {
        version: DESKTOP_APP_ACTIVATION_PROTOCOL_VERSION,
        request_id: &request_id,
        request_type: "open-workspace",
        workspace_root: &workspace_root,
        platform: platform_wire_name(platform),
    }) else {
        eprintln!("Failed to encode desktop activation request.");
        return 1;
    };

    for state_dir in state_dirs {
        let state_str = state_dir.to_string_lossy();
        let target = resolve_desktop_app_control_address(&state_str, platform, &temp_dir, uid);
        if !unix_dir_is_safe(target.directory.as_deref(), uid) {
            continue;
        }
        match exchange_json_line(
            &target.address,
            &payload,
            MAX_REQUEST_BYTES,
            MAX_RESPONSE_BYTES,
            IPC_TIMEOUT,
        ) {
            Ok(line) => return handle_activation_reply(&line, &request_id, &workspace_root),
            Err(PipeExchangeError::Unreachable) => {}
            Err(PipeExchangeError::Protocol) => {
                eprintln!("DesktopAppUnreachableError: The desktop app response is invalid.");
                return 1;
            }
        }
    }

    eprintln!(
        "DesktopAppUnreachableError: Could not reach the LegionCode desktop app. Start or update the desktop app on this machine, then run `legioncode app` again. A running LegionCode server is not enough."
    );
    1
}

fn parse_app_flags(rest: &[String]) -> Option<AppFlags> {
    let mut base_dir = None;
    let mut workspace = None;
    let mut idx = 0;

    while idx < rest.len() {
        let arg = &rest[idx];
        if let Ok(Some(val)) = take_flag_value(rest, &mut idx, "base-dir") {
            base_dir = Some(val);
        } else if arg.starts_with('-') || workspace.is_some() {
            return None;
        } else {
            workspace = Some(arg.clone());
        }
        idx += 1;
    }

    Some(AppFlags {
        base_dir,
        workspace,
    })
}

fn resolve_workspace_root(raw: Option<&str>) -> String {
    let candidate = match raw.map(str::trim).filter(|s| !s.is_empty()) {
        Some(path) => resolve_platform_path(&expand_home(path)),
        None => env::current_dir().map_or_else(
            |_| resolve_platform_path(Path::new(".")),
            |dir| resolve_platform_path(&dir),
        ),
    };
    candidate.to_string_lossy().into_owned()
}

fn handle_activation_reply(line: &str, request_id: &str, workspace_root: &str) -> i32 {
    let Ok(reply) = serde_json::from_str::<DesktopActivationResponse>(line) else {
        eprintln!("DesktopAppUnreachableError: The desktop app response is invalid.");
        return 1;
    };
    if reply
        .version
        .unwrap_or(DESKTOP_APP_ACTIVATION_PROTOCOL_VERSION)
        != DESKTOP_APP_ACTIVATION_PROTOCOL_VERSION
        || reply.request_id != request_id
    {
        eprintln!("DesktopAppUnreachableError: The desktop app response is invalid.");
        return 1;
    }

    let is_activated = match reply.status.as_deref() {
        Some("activated") => true,
        Some("error") => false,
        Some(_) => {
            eprintln!("DesktopAppUnreachableError: The desktop app response is invalid.");
            return 1;
        }
        None => match reply.ok {
            Some(true) => true,
            Some(false) => false,
            None => {
                eprintln!("DesktopAppUnreachableError: The desktop app response is invalid.");
                return 1;
            }
        },
    };

    if is_activated {
        return write_stdout_line(
            &format!("Opened {workspace_root} in LegionCode."),
            "app activation",
        );
    }

    let code = reply.code.as_deref().unwrap_or("request-failed");
    let detail = reply
        .message
        .as_deref()
        .filter(|m| !m.is_empty())
        .unwrap_or("The desktop app rejected the request.");
    eprintln!("DesktopAppRequestFailedError ({code}): {detail}");
    1
}

const fn platform_wire_name(platform: DesktopControlPlatform) -> &'static str {
    match platform {
        DesktopControlPlatform::Win32 => "win32",
        DesktopControlPlatform::Linux => "linux",
        DesktopControlPlatform::Darwin => "darwin",
    }
}

fn unix_dir_is_safe(directory: Option<&str>, uid: Option<u32>) -> bool {
    #[cfg(unix)]
    {
        let Some(dir) = directory else {
            return true;
        };
        crate::pipe_ipc::validate_unix_socket_dir(Path::new(dir), uid)
    }
    #[cfg(not(unix))]
    {
        let _ = (directory, uid);
        true
    }
}
