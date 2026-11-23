//! `legioncode doctor` command inspecting and compacting `statev2.sqlite`.

use std::io::{BufWriter, IsTerminal as _, Write};
use std::time::{Duration, Instant};

use legion_persistence::maintenance::remove_legacy_database;

use crate::cli_global::fail_with_command_help;

use legion_persistence::maintenance::{
    DatabaseCompactStep, DoctorOptions, DoctorRepairId, MaintenanceError,
    compact_database_with_steps, format_bytes, inspect_database, plan_doctor_repairs,
    render_doctor_report,
};
use legion_persistence::ownership::ServerOwnerLock;

use crate::fallback::{delegate_to_node, derive_paths, take_flag_value, unit_forced_original};

/// Unit identifier for `RUST_ANYTHING_UNIT_DOCTOR` and fallback logs.
pub const UNIT_DOCTOR: &str = "DOCTOR";

#[derive(Debug, Clone, PartialEq, Eq)]
struct DoctorFlags {
    base_dir: Option<String>,
    fix: bool,
    dry_run: bool,
}

/// Runs `legioncode doctor`, inspecting SQLite and compacting or deleting legacy DBs without Node delegation.
pub fn run(args: &[String]) -> i32 {
    if unit_forced_original(UNIT_DOCTOR) {
        return delegate_to_node(UNIT_DOCTOR, args);
    }

    let flags = match parse_doctor_flags(args.get(1..).unwrap_or(&[])) {
        Ok(parsed) => parsed,
        Err(code) => return code,
    };

    let (_, paths) = derive_paths(flags.base_dir.as_deref());
    let status = match inspect_database(&paths.db_path) {
        Ok(status) => status,
        Err(MaintenanceError::DatabaseMissing { .. }) => {
            eprintln!(
                "DatabaseMissingError: No existing LegionCode database was found in this data directory."
            );
            return 1;
        }
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };

    let interactive = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let report = render_doctor_report(
        &status,
        DoctorOptions {
            fix: flags.fix,
            dry_run: flags.dry_run,
            interactive,
        },
    );
    let mut out = BufWriter::with_capacity(8192, std::io::stdout().lock());
    if let Err(err) = writeln!(out, "{report}").and_then(|()| out.flush()) {
        eprintln!("Failed to write doctor report: {err}");
        return 1;
    }
    drop(out);

    if flags.dry_run {
        return 0;
    }
    let repairs = plan_doctor_repairs(&status);
    if repairs.is_empty() {
        return 0;
    }
    if !flags.fix && !interactive {
        return 0;
    }

    let (run_db, run_legacy) = if flags.fix {
        let has_rec_db = repairs
            .iter()
            .any(|r| r.id == DoctorRepairId::Database && r.recommended);
        if !has_rec_db {
            println!("No recommended repairs; nothing changed.");
            return 0;
        }
        (true, false)
    } else {
        select_interactive_repairs(&repairs)
    };

    if !run_db && !run_legacy {
        println!("No repairs selected; nothing changed.");
        return 0;
    }

    if run_db {
        let code = run_database_repair(&paths.server_runtime_state_path, &paths.db_path);
        if code != 0 {
            return code;
        }
    }
    if run_legacy {
        match remove_legacy_database(&paths.db_path) {
            Ok(_) => println!("\nDeleted the database from before 0.1.0."),
            Err(err) => {
                eprintln!("{err}");
                return 1;
            }
        }
    }
    0
}

fn run_database_repair(state_path: &std::path::Path, db_path: &std::path::Path) -> i32 {
    let owner_lock = match ServerOwnerLock::hold_lock_only(state_path) {
        Ok(lock) => lock,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };

    println!("\nReclaiming database space. Each step is shown as it starts.");
    let started_at = Instant::now();
    let result = compact_database_with_steps(db_path, |step| {
        let elapsed = format_elapsed(started_at.elapsed());
        println!("  [{elapsed}] {}", compact_step_label(step));
    });
    let _ = owner_lock.release();

    match result {
        Ok(summary) => {
            let elapsed = format_elapsed(started_at.elapsed());
            println!(
                "Done in {elapsed}: {} -> {}, {} superseded events removed.",
                format_bytes(summary.before_bytes),
                format_bytes(summary.after_bytes),
                summary.deleted_event_count
            );
            0
        }
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}

fn select_interactive_repairs(
    repairs: &[legion_persistence::maintenance::DoctorRepair],
) -> (bool, bool) {
    let mut run_db = false;
    let mut run_legacy = false;
    for repair in repairs {
        let default_yes = repair.recommended;
        let prompt_hint = if default_yes { "[Y/n]" } else { "[y/N]" };
        print!("\n{}? {prompt_hint} ", repair.title);
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line).is_err() {
            return (false, false);
        }
        let ans = line.trim().to_ascii_lowercase();
        let chosen = if ans.is_empty() {
            default_yes
        } else {
            matches!(ans.as_str(), "y" | "yes")
        };
        match repair.id {
            DoctorRepairId::Database if chosen => run_db = true,
            DoctorRepairId::Legacy if chosen => run_legacy = true,
            _ => {}
        }
    }
    (run_db, run_legacy)
}

fn parse_doctor_flags(rest: &[String]) -> Result<DoctorFlags, i32> {
    let path = &["doctor"];
    let mut base_dir = None;
    let mut fix = false;
    let mut dry_run = false;
    let mut idx = 0;

    while idx < rest.len() {
        let arg = &rest[idx];
        if arg == "--fix" {
            fix = true;
        } else if arg == "--dry-run" {
            dry_run = true;
        } else {
            match take_flag_value(rest, &mut idx, "base-dir") {
                Ok(Some(val)) => base_dir = Some(val),
                Ok(None) if arg.starts_with('-') => {
                    return Err(fail_with_command_help(
                        path,
                        &format!("Unrecognized flag: {arg} in command legioncode doctor"),
                    ));
                }
                Ok(None) => {
                    return Err(fail_with_command_help(
                        path,
                        &format!("Unexpected argument: {arg}"),
                    ));
                }
                Err(()) => {
                    return Err(fail_with_command_help(
                        path,
                        "Missing value for flag --base-dir",
                    ));
                }
            }
        }
        idx += 1;
    }

    Ok(DoctorFlags {
        base_dir,
        fix,
        dry_run,
    })
}

const fn compact_step_label(step: DatabaseCompactStep) -> &'static str {
    match step {
        DatabaseCompactStep::PruneEvents => "Removing superseded events",
        DatabaseCompactStep::Vacuum => "Rewriting the database file",
        DatabaseCompactStep::RebuildSearch => "Rebuilding the search index",
        DatabaseCompactStep::Checkpoint => "Flushing the write-ahead log",
    }
}

fn format_elapsed(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    format!("{}:{:02}", secs / 60, secs % 60)
}
