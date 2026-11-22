//! Fast synchronous `legioncode __service-preflight --database-path <path> --launcher-protocol <int>` command.

use std::path::Path;

use legion_persistence::service_preflight::run_service_preflight;

use crate::cli_global::fail_with_command_help;
use crate::fallback::{delegate_to_node, take_flag_value, unit_forced_original, write_stdout_line};

/// Unit identifier for `RUST_ANYTHING_UNIT_SERVICE_PREFLIGHT` and fallback logs.
pub const UNIT_SERVICE_PREFLIGHT: &str = "SERVICE_PREFLIGHT";

#[derive(Debug, Clone, PartialEq, Eq)]
struct PreflightFlags {
    database_path: String,
    launcher_protocol: i64,
}

/// Runs `legioncode __service-preflight` natively unless `RUST_ANYTHING_UNIT_SERVICE_PREFLIGHT=original`.
pub fn run(args: &[String]) -> i32 {
    if unit_forced_original(UNIT_SERVICE_PREFLIGHT) {
        return delegate_to_node(UNIT_SERVICE_PREFLIGHT, args);
    }

    let flags = match parse_preflight_flags(args.get(1..).unwrap_or(&[])) {
        Ok(flags) => flags,
        Err(code) => return code,
    };

    let result = run_service_preflight(
        Path::new(&flags.database_path),
        flags.launcher_protocol,
        env!("CARGO_PKG_VERSION"),
    );

    match serde_json::to_string(&result) {
        Ok(json) => write_stdout_line(&json, "service preflight result"),
        Err(err) => {
            eprintln!("Failed to serialize service preflight result: {err}");
            1
        }
    }
}

fn parse_preflight_flags(rest: &[String]) -> Result<PreflightFlags, i32> {
    let path = &["__service-preflight"];
    let mut database_path = None;
    let mut launcher_protocol = None;
    let mut idx = 0;

    while idx < rest.len() {
        let arg = &rest[idx];
        match take_flag_value(rest, &mut idx, "database-path") {
            Ok(Some(val)) => database_path = Some(val),
            Err(()) => {
                return Err(fail_with_command_help(
                    path,
                    "Missing value for flag --database-path",
                ));
            }
            Ok(None) => match take_flag_value(rest, &mut idx, "launcher-protocol") {
                Ok(Some(raw_proto)) => {
                    let parsed = raw_proto.trim().parse::<i64>().map_err(|_| {
                        fail_with_command_help(
                            path,
                            &format!(
                                "Invalid value for flag --launcher-protocol: \"{raw_proto}\"."
                            ),
                        )
                    })?;
                    launcher_protocol = Some(parsed);
                }
                Err(()) => {
                    return Err(fail_with_command_help(
                        path,
                        "Missing value for flag --launcher-protocol",
                    ));
                }
                Ok(None) => {
                    return Err(fail_with_command_help(
                        path,
                        &format!("Unrecognized argument: {arg}"),
                    ));
                }
            },
        }
        idx += 1;
    }

    let Some(database_path) = database_path else {
        return Err(fail_with_command_help(
            path,
            "Missing required flag: --database-path",
        ));
    };
    let Some(launcher_protocol) = launcher_protocol else {
        return Err(fail_with_command_help(
            path,
            "Missing required flag: --launcher-protocol",
        ));
    };
    Ok(PreflightFlags {
        database_path,
        launcher_protocol,
    })
}
