//! Human-readable `t3 status` formatter matching `apps/server/src/cli/up.ts`.

use t3_edge::boot_service_units::format_boot_service_base_dir_option;

use crate::cmd_status::StatusOutputJson;
use crate::fallback::write_stdout_line;
use crate::invocation::resolve_host_cli_command;

/// Renders and prints human-readable `t3 status` output matching `cli/up.ts:146-200,558-563`.
pub(crate) fn print_human_status(raw_base_dir: Option<&str>, status: &StatusOutputJson) -> i32 {
    let rendered = format_human_status(raw_base_dir, status);
    write_stdout_line(&rendered, "status output")
}

pub(crate) fn format_human_status(raw_base_dir: Option<&str>, status: &StatusOutputJson) -> String {
    if let Some(other) = &status.service.other_base_dir {
        let escaped_option = format_boot_service_base_dir_option(other);
        return format!(
            "This machine\n  {:<15}installed for another T3 home \u{00B7} {other}\n\nPass `{escaped_option}` to manage it.",
            "Service"
        );
    }

    let base_dir_args = raw_base_dir
        .map(|dir| vec!["--base-dir".to_owned(), dir.to_owned()])
        .unwrap_or_default();

    let service_value = if !status.service.supported {
        "unavailable".to_owned()
    } else if status.service.installed {
        let ver = status
            .service
            .installed_version
            .as_deref()
            .unwrap_or(env!("CARGO_PKG_VERSION"));
        format!("installed \u{00B7} t3@{ver}")
    } else {
        "not installed".to_owned()
    };

    let lan_value = if let Some(host) = &status.lan.host_override {
        format!("overridden \u{00B7} T3CODE_HOST={host}")
    } else if status.lan.inherited_default {
        "on \u{00B7} kept from an older install".to_owned()
    } else if status.lan.enabled {
        "on \u{00B7} pairing code only".to_owned()
    } else {
        "off".to_owned()
    };

    let mut lines = vec![
        "This machine".to_owned(),
        format!("  {:<15}{service_value}", "Service"),
        format!("  {:<15}{lan_value}", "Local network"),
    ];
    if let Some(connect) = &status.connect {
        let revoked = connect
            .get("revoked")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let enabled = connect
            .get("enabled")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let connect_val = if revoked {
            "revoked"
        } else if enabled {
            "on"
        } else {
            "off"
        };
        lines.push(format!("  {:<15}{connect_val}", "T3 Connect"));
    }

    let mut notes = Vec::new();
    if status.lan.host_override.is_none() && status.lan.enabled {
        notes.push(
            "Anyone on your LAN or tailnet can reach the port. A pairing code is the only lock."
                .to_owned(),
        );
    }
    if status.lan.inherited_default {
        let mut no_lan_args = base_dir_args.clone();
        no_lan_args.push("--no-lan".to_owned());
        let up_no_lan_hint = resolve_host_cli_command("up", &no_lan_args);
        notes.push(format!(
            "Older installs keep LAN access on. Turn it off: `{up_no_lan_hint}`."
        ));
    }

    if !notes.is_empty() {
        lines.push(String::new());
        lines.extend(notes);
    }

    let up_hint = resolve_host_cli_command("up", &base_dir_args);
    lines.push(String::new());
    lines.push(format!("Change access: `{up_hint}`."));
    lines.join("\n")
}
