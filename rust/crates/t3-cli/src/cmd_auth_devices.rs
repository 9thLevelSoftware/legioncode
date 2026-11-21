//! SQLite + live-server operations for `t3 auth connect-device list|approve|revoke`.

use t3_persistence::auth_connect_devices::{
    ConnectApproveOutcome, ConnectRevokeOutcome, approve_connect_device_by_code,
    format_connect_device_list, list_connect_devices, revoke_connect_device_by_code,
};
use t3_persistence::ownership::read_live_server_runtime_state;

use crate::cmd_auth_live::try_live_connect_device_post;
use crate::cmd_auth_ops::{derive_auth_paths, open_auth_store};
use crate::fallback::write_stdout_line;

pub(crate) fn exec_connect_device_list(
    base_dir: Option<&str>,
    dev_url: Option<&str>,
    json: bool,
) -> i32 {
    let paths = derive_auth_paths(base_dir, dev_url);
    let Ok(store) = open_auth_store(&paths) else {
        return 1;
    };
    let (waiting, trusted) = match list_connect_devices(store.connection()) {
        Ok(pair) => pair,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };
    match format_connect_device_list(&waiting, &trusted, json) {
        Ok(rendered) => write_stdout_line(&rendered, "auth output"),
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}

pub(crate) fn exec_connect_device_approve(
    base_dir: Option<&str>,
    dev_url: Option<&str>,
    code_arg: &str,
) -> i32 {
    let paths = derive_auth_paths(base_dir, dev_url);
    if let Some(runtime) = read_live_server_runtime_state(&paths.server_runtime_state_path)
        && let Some(code) = try_live_connect_device_post(
            &paths,
            &runtime.origin,
            "/api/auth/connect-devices/approve",
            code_arg,
            &format!("Approved {code_arg}. The device connects on its next try.\n"),
            &format!("No device waiting for approval shows {code_arg}.\n"),
            &format!("No device waiting for approval shows {code_arg}.\n"),
        )
    {
        return code;
    }

    let Ok(store) = open_auth_store(&paths) else {
        return 1;
    };
    match approve_connect_device_by_code(store.connection(), code_arg) {
        Ok(ConnectApproveOutcome::Approved { .. }) => write_stdout_line(
            &format!("Approved {code_arg}. The device connects on its next try.\n"),
            "auth",
        ),
        Ok(ConnectApproveOutcome::NotFound | ConnectApproveOutcome::Ambiguous { .. }) => {
            write_stdout_line(
                &format!("No device waiting for approval shows {code_arg}.\n"),
                "auth",
            )
        }
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}

pub(crate) fn exec_connect_device_revoke(
    base_dir: Option<&str>,
    dev_url: Option<&str>,
    code_arg: &str,
) -> i32 {
    let paths = derive_auth_paths(base_dir, dev_url);
    if let Some(runtime) = read_live_server_runtime_state(&paths.server_runtime_state_path)
        && let Some(code) = try_live_connect_device_post(
            &paths,
            &runtime.origin,
            "/api/auth/connect-devices/revoke",
            code_arg,
            &format!(
                "Revoked {code_arg}. If approval for new devices is off, it can connect again on its own.\n"
            ),
            &format!("No trusted device shows {code_arg}.\n"),
            &format!("More than one trusted device shows {code_arg}; revoke it from a client.\n"),
        )
    {
        return code;
    }

    let Ok(store) = open_auth_store(&paths) else {
        return 1;
    };
    match revoke_connect_device_by_code(store.connection(), code_arg) {
        Ok(ConnectRevokeOutcome::Revoked { .. }) => write_stdout_line(
            &format!(
                "Revoked {code_arg}. If approval for new devices is off, it can connect again on its own.\n"
            ),
            "auth",
        ),
        Ok(ConnectRevokeOutcome::NotFound) => {
            write_stdout_line(&format!("No trusted device shows {code_arg}.\n"), "auth")
        }
        Ok(ConnectRevokeOutcome::Ambiguous { .. }) => write_stdout_line(
            &format!("More than one trusted device shows {code_arg}; revoke it from a client.\n"),
            "auth",
        ),
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}
