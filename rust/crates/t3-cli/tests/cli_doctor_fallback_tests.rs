//! Integration tests for `doctor` (`--dry-run`, `--fix`, missing DB, lock contention) and
//! `RUST_ANYTHING_UNIT_<UNIT>=original` / `RUST_ANYTHING_FALLBACK_LOG` delegation.

mod support;

use std::fs;

use serde_json::Value;
use t3_persistence::maintenance::inspect_database;
use t3_persistence::ownership::ServerOwnerLock;
use t3_persistence::paths::ServerDerivedPaths;
use t3_persistence::sqlite::SqliteStore;

use support::{TestResult, isolated_env, run_t3};

#[test]
fn doctor_handles_missing_clean_dry_run_and_fix() -> TestResult {
    let temp = tempfile::tempdir()?;
    let env = isolated_env(temp.path(), &[]);
    let base_missing = temp.path().join("missing-home");

    let out_missing = run_t3(
        &[
            "doctor",
            "--base-dir",
            &base_missing.to_string_lossy(),
            "--dry-run",
        ],
        &env,
    )?;
    assert_eq!(out_missing.status.code(), Some(1));
    assert_eq!(
        String::from_utf8(out_missing.stderr)?,
        "DatabaseMissingError: No existing T3 Code database was found in this data directory.\n"
    );

    // Clean DB -> "Nothing to repair."
    let base_clean = temp.path().join("clean-home");
    let clean_paths = ServerDerivedPaths::derive(&base_clean, false, true);
    SqliteStore::open(&clean_paths.db_path)?.close()?;
    let out_clean = run_t3(
        &[
            "doctor",
            "--base-dir",
            &base_clean.to_string_lossy(),
            "--dry-run",
        ],
        &env,
    )?;
    assert!(out_clean.status.success());
    let clean_stdout = String::from_utf8(out_clean.stdout)?;
    assert!(clean_stdout.contains("Nothing to repair.\n"));

    // Fragmented DB (auto_vacuum = NONE + dropped table) -> dry-run reports repair, --fix compacts.
    let base_fix = temp.path().join("fix-home");
    let fix_paths = ServerDerivedPaths::derive(&base_fix, false, true);
    let store = SqliteStore::open(&fix_paths.db_path)?;
    store.connection().execute_batch(
        "PRAGMA auto_vacuum = NONE;
         VACUUM;
         CREATE TABLE discard_blob (b BLOB);
         INSERT INTO discard_blob VALUES (zeroblob(524288));
         DROP TABLE discard_blob;
         PRAGMA wal_checkpoint(TRUNCATE);",
    )?;
    store.close()?;

    let out_dry = run_t3(
        &[
            "doctor",
            "--base-dir",
            &base_fix.to_string_lossy(),
            "--dry-run",
        ],
        &env,
    )?;
    assert!(out_dry.status.success());
    let dry_stdout = String::from_utf8(out_dry.stdout)?;
    assert!(dry_stdout.contains("Reclaim database space (recommended)"));
    assert!(dry_stdout.contains("Dry run: nothing changed."));
    assert!(!inspect_database(&fix_paths.db_path)?.incremental_vacuum_enabled);

    // Contended state directory lock prevents doctor --fix from touching DB.
    let owner = ServerOwnerLock::acquire(&fix_paths.server_runtime_state_path)?;
    let out_contended = run_t3(
        &["doctor", "--base-dir", &base_fix.to_string_lossy(), "--fix"],
        &env,
    )?;
    assert_eq!(out_contended.status.code(), Some(1));
    owner.release()?;

    let out_fix = run_t3(
        &["doctor", "--base-dir", &base_fix.to_string_lossy(), "--fix"],
        &env,
    )?;
    assert!(out_fix.status.success());
    let fix_stdout = String::from_utf8(out_fix.stdout)?;
    for step in [
        "Removing superseded events",
        "Rewriting the database file",
        "Rebuilding the search index",
        "Flushing the write-ahead log",
    ] {
        assert!(
            fix_stdout.contains(step),
            "missing step `{step}` in:\n{fix_stdout}"
        );
    }
    assert!(fix_stdout.contains("Done in "));
    assert!(fix_stdout.contains("superseded events removed."));

    let after = inspect_database(&fix_paths.db_path)?;
    assert!(after.incremental_vacuum_enabled);
    assert_eq!(after.reclaimable_bytes, 0);
    Ok(())
}

#[test]
fn unit_forced_original_and_passthrough_log_and_delegate_to_node() -> TestResult {
    let temp = tempfile::tempdir()?;
    let log_path = temp.path().join("logs").join("fallback.jsonl");
    let log_str = log_path.to_string_lossy().into_owned();
    let base_dir = temp.path().join("t3home");
    let base_str = base_dir.to_string_lossy().into_owned();

    // 1. RUST_ANYTHING_UNIT_STATUS=original delegates `status --json` to Node and records unit "STATUS".
    let env_forced = isolated_env(
        temp.path(),
        &[
            ("RUST_ANYTHING_UNIT_STATUS", "Original"),
            ("RUST_ANYTHING_FALLBACK_LOG", &log_str),
        ],
    );
    let out_status = run_t3(&["status", "--base-dir", &base_str, "--json"], &env_forced)?;
    assert!(out_status.status.success());
    let status_json: Value = serde_json::from_str(&String::from_utf8(out_status.stdout)?)?;
    assert!(status_json.get("service").is_some());

    // 2. `--completions bash` runs natively without writing to the fallback log.
    let empty_bin = temp.path().join("empty-bin");
    fs::create_dir_all(&empty_bin)?;
    let empty_bin_str = empty_bin.to_string_lossy().into_owned();
    let env_native = isolated_env(
        temp.path(),
        &[
            ("RUST_ANYTHING_FALLBACK_LOG", &log_str),
            ("PATH", &empty_bin_str),
            ("T3_BIN_TS", r"Z:\nonexistent\bin.ts"),
        ],
    );
    let out_completions = run_t3(&["--completions", "bash"], &env_native)?;
    assert!(out_completions.status.success());
    assert!(String::from_utf8(out_completions.stdout)?.contains("t3"));

    // 3. Invalid flags (`serve --share`, `status --unknown`) and unknown bare word reject natively (exit 1, 0 fallback).
    assert_eq!(
        run_t3(&["serve", "--share"], &env_native)?.status.code(),
        Some(1)
    );
    assert_eq!(
        run_t3(&["status", "--unknown"], &env_native)?.status.code(),
        Some(1)
    );
    assert_eq!(run_t3(&["bogus-cmd"], &env_native)?.status.code(), Some(1));

    // Only the forced-original STATUS call in step 1 should be logged so far.
    let pre_ks_count = fs::read_to_string(&log_path)?.lines().count();
    assert_eq!(pre_ks_count, 1);

    // 4. Forced-original kill switches record exact unit names.
    verify_forced_unit_kill_switches(temp.path(), &empty_bin_str, &log_path, &base_str)
}

fn verify_forced_unit_kill_switches(
    home: &std::path::Path,
    empty_bin_str: &str,
    log_path: &std::path::Path,
    base_str: &str,
) -> TestResult {
    let log_str = log_path.to_string_lossy().into_owned();
    let env_all_forced = isolated_env(
        home,
        &[
            ("RUST_ANYTHING_FALLBACK_LOG", &log_str),
            ("PATH", empty_bin_str),
            ("RUST_ANYTHING_UNIT_VERSION", "original"),
            ("RUST_ANYTHING_UNIT_POLICY", "ORIGINAL"),
            ("RUST_ANYTHING_UNIT_DOCTOR", "OrIgInAl"),
            ("RUST_ANYTHING_UNIT_SERVE", "original"),
            ("RUST_ANYTHING_UNIT_COMPLETIONS", "original"),
            ("RUST_ANYTHING_UNIT_CLI_PASSTHROUGH", "original"),
        ],
    );
    for cmd in [
        &["--version"][..],
        &["policy", "check", "x.json"],
        &["doctor"],
        &["serve", "--share"],
        &["--completions", "bash"],
        &["bogus-cmd"],
    ] {
        assert_eq!(run_t3(cmd, &env_all_forced)?.status.code(), Some(1));
    }

    let log_lines: Vec<Value> = fs::read_to_string(log_path)?
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<Vec<_>, _>>()?;
    let expected: &[(&str, Value)] = &[
        (
            "STATUS",
            serde_json::json!(["status", "--base-dir", base_str, "--json"]),
        ),
        ("VERSION", serde_json::json!(["--version"])),
        ("POLICY", serde_json::json!(["policy", "check", "x.json"])),
        ("DOCTOR", serde_json::json!(["doctor"])),
        ("SERVE", serde_json::json!(["serve", "--share"])),
        ("COMPLETIONS", serde_json::json!(["--completions", "bash"])),
        ("CLI_PASSTHROUGH", serde_json::json!(["bogus-cmd"])),
    ];
    assert_eq!(log_lines.len(), expected.len());
    for (entry, (unit, args)) in log_lines.iter().zip(expected) {
        assert_eq!(entry["unit"], *unit);
        assert_eq!(&entry["args"], args);
    }
    Ok(())
}
