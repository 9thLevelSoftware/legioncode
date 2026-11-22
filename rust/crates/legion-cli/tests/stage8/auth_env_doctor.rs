//! Zero-Node integration tests for `legioncode auth` (`pairing`, `session`, `connect-device`)
//! and non-TTY `legioncode doctor` / legacy DB cleanup.

use std::fs;

use legion_persistence::auth_connect_devices::connect_device_verification_code;
use legion_persistence::maintenance::{inspect_database, remove_legacy_database};
use legion_persistence::paths::ServerDerivedPaths;
use legion_persistence::sqlite::SqliteStore;
use serde_json::Value;

use crate::support::{TestResult, isolated_env, run_legioncode};

const NONEXISTENT_BIN_TS: &str = r"Z:\nonexistent\bin.ts";

#[test]
fn auth_pairing_session_and_connect_device_lifecycles_zero_node() -> TestResult {
    let temp = tempfile::tempdir()?;
    let empty_bin = temp.path().join("empty-bin");
    fs::create_dir_all(&empty_bin)?;
    let empty_str = empty_bin.to_string_lossy().into_owned();
    let log_file = temp.path().join("auth-stage8-fallback.jsonl");
    let log_str = log_file.to_string_lossy().into_owned();
    let base_dir = temp.path().join("auth-home");
    let base_str = base_dir.to_string_lossy().into_owned();
    let env = isolated_env(
        temp.path(),
        &[
            ("PATH", &empty_str),
            ("LEGIONCODE_BIN_TS", NONEXISTENT_BIN_TS),
            ("RUST_ANYTHING_FALLBACK_LOG", &log_str),
        ],
    );

    exercise_pairing_lifecycle(&base_str, &env)?;

    // 2. Session issue (--token-only and --json) -> list -> revoke.
    let tok_only = run_legioncode(
        &[
            "auth",
            "session",
            "issue",
            "--token-only",
            "--label",
            "TokOnly",
            "--base-dir",
            &base_str,
        ],
        &env,
    )?;
    assert_eq!(tok_only.status.code(), Some(0));
    let raw_tok = String::from_utf8(tok_only.stdout)?;
    assert_ne!(raw_tok.trim(), "");

    let iss_json = run_legioncode(
        &[
            "auth",
            "session",
            "issue",
            "--json",
            "--label",
            "Stage8 Session",
            "--base-dir",
            &base_str,
        ],
        &env,
    )?;
    assert_eq!(iss_json.status.code(), Some(0));
    let ses: Value = serde_json::from_slice(&iss_json.stdout)?;
    let ses_id = ses["sessionId"]
        .as_str()
        .ok_or("missing sessionId")?
        .to_owned();

    let rev_s = run_legioncode(
        &[
            "auth",
            "session",
            "revoke",
            &ses_id,
            "--base-dir",
            &base_str,
        ],
        &env,
    )?;
    assert_eq!(rev_s.status.code(), Some(0));
    assert!(String::from_utf8(rev_s.stdout)?.contains(&format!("Revoked session {ses_id}.")));

    exercise_connect_device(&base_dir, &base_str, &env)?;
    assert!(!log_file.exists(), "fallback log must stay absent");
    Ok(())
}

fn exercise_pairing_lifecycle(base_str: &str, env: &[(&str, &str)]) -> TestResult {
    let create = run_legioncode(
        &[
            "auth",
            "pairing",
            "create",
            "--label",
            "Stage8 Pair",
            "--ttl",
            "15m",
            "--json",
            "--base-dir",
            base_str,
        ],
        env,
    )?;
    assert_eq!(create.status.code(), Some(0));
    let created: Value = serde_json::from_slice(&create.stdout)?;
    let pair_id = created["id"]
        .as_str()
        .ok_or("missing pairing id")?
        .to_owned();
    assert_eq!(created["label"], "Stage8 Pair");

    let list_p = run_legioncode(
        &["auth", "pairing", "list", "--json", "--base-dir", base_str],
        env,
    )?;
    assert_eq!(list_p.status.code(), Some(0));
    let listed_p: Vec<Value> = serde_json::from_slice(&list_p.stdout)?;
    assert_eq!(listed_p.len(), 1);
    assert_eq!(listed_p[0]["id"], pair_id);

    let rev_p = run_legioncode(
        &[
            "auth",
            "pairing",
            "revoke",
            &pair_id,
            "--base-dir",
            base_str,
        ],
        env,
    )?;
    assert_eq!(rev_p.status.code(), Some(0));
    assert!(
        String::from_utf8(rev_p.stdout)?
            .contains(&format!("Revoked pairing credential {pair_id}."))
    );
    Ok(())
}

fn exercise_connect_device(
    base_dir: &std::path::Path,
    base_str: &str,
    env: &[(&str, &str)],
) -> TestResult {
    // 3. Connect-device list -> approve -> list -> revoke.
    let pending = run_legioncode(
        &[
            "auth",
            "pairing",
            "create",
            "--label",
            "Connect Pending",
            "--json",
            "--base-dir",
            base_str,
        ],
        env,
    )?;
    let pending_val: Value = serde_json::from_slice(&pending.stdout)?;
    let pending_id = pending_val["id"].as_str().ok_or("missing pending id")?;
    let thumb = "AbCdEf12";
    let paths = ServerDerivedPaths::derive(base_dir, false, true);
    let store = SqliteStore::open(&paths.db_path)?;
    store.connection().execute(
        "UPDATE auth_pairing_links SET requires_connect_approval = 1, proof_key_thumbprint = ?1 WHERE id = ?2",
        rusqlite::params![thumb, pending_id],
    )?;
    store.close()?;

    let vcode = connect_device_verification_code(thumb);
    let cd_list_1 = run_legioncode(
        &[
            "auth",
            "connect-device",
            "list",
            "--json",
            "--base-dir",
            base_str,
        ],
        env,
    )?;
    assert_eq!(cd_list_1.status.code(), Some(0));
    let cd_list_1_text = String::from_utf8(cd_list_1.stdout)?;
    assert!(cd_list_1_text.ends_with("\n\n"));
    let cd1: Value = serde_json::from_str(&cd_list_1_text)?;
    assert_eq!(cd1["waiting"][0]["verificationCode"], vcode);

    let cd_app = run_legioncode(
        &[
            "auth",
            "connect-device",
            "approve",
            &vcode,
            "--base-dir",
            base_str,
        ],
        env,
    )?;
    assert_eq!(cd_app.status.code(), Some(0));
    assert!(String::from_utf8(cd_app.stdout)?.contains(&format!("Approved {vcode}.")));

    let cd_list_2 = run_legioncode(
        &[
            "auth",
            "connect-device",
            "list",
            "--json",
            "--base-dir",
            base_str,
        ],
        env,
    )?;
    let cd2: Value = serde_json::from_slice(&cd_list_2.stdout)?;
    assert_eq!(cd2["trusted"][0]["verificationCode"], vcode);

    let cd_rev = run_legioncode(
        &[
            "auth",
            "connect-device",
            "revoke",
            &vcode,
            "--base-dir",
            base_str,
        ],
        env,
    )?;
    assert_eq!(cd_rev.status.code(), Some(0));
    assert!(String::from_utf8(cd_rev.stdout)?.contains(&format!("Revoked {vcode}.")));
    Ok(())
}

fn legioncode_ok(
    args: &[&str],
    env: &[(&str, &str)],
) -> Result<String, Box<dyn std::error::Error>> {
    let out = run_legioncode(args, env)?;
    assert_eq!(
        out.status.code(),
        Some(0),
        "args={args:?} stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(String::from_utf8(out.stdout)?)
}

#[test]
fn non_tty_doctor_hint_and_legacy_cleanup_zero_node() -> TestResult {
    let temp = tempfile::tempdir()?;
    let empty_bin = temp.path().join("empty-bin");
    fs::create_dir_all(&empty_bin)?;
    let empty_str = empty_bin.to_string_lossy().into_owned();
    let log_file = temp.path().join("doc-stage8-fallback.jsonl");
    let log_str = log_file.to_string_lossy().into_owned();
    let env = isolated_env(
        temp.path(),
        &[
            ("PATH", &empty_str),
            ("LEGIONCODE_BIN_TS", NONEXISTENT_BIN_TS),
            ("RUST_ANYTHING_FALLBACK_LOG", &log_str),
        ],
    );

    let doc_home = temp.path().join("doctor-home");
    let doc_str = doc_home.to_string_lossy().into_owned();
    let doc_paths = ServerDerivedPaths::derive(&doc_home, false, true);
    let store = SqliteStore::open(&doc_paths.db_path)?;
    store.connection().execute_batch(
        "PRAGMA auto_vacuum = NONE; VACUUM; CREATE TABLE bloat (b BLOB); INSERT INTO bloat VALUES (zeroblob(262144)); DROP TABLE bloat; PRAGMA wal_checkpoint(TRUNCATE);",
    )?;
    store.close()?;

    let legacy_path = doc_paths
        .db_path
        .parent()
        .ok_or("missing db parent")?
        .join("state.sqlite");
    fs::copy(&doc_paths.db_path, &legacy_path)?;

    let doc_stdout = legioncode_ok(&["doctor", "--base-dir", &doc_str], &env)?;
    assert!(
        doc_stdout.contains(
            "Run in a terminal to choose repairs, or pass --fix for the recommended ones."
        )
    );
    assert!(doc_stdout.contains("Database from before 0.1.0:"));

    let _ = legioncode_ok(&["doctor", "--fix", "--base-dir", &doc_str], &env)?;
    assert!(remove_legacy_database(&doc_paths.db_path)?);
    assert!(!legacy_path.exists());
    assert!(
        inspect_database(&doc_paths.db_path)?
            .legacy_database_bytes
            .is_none()
    );

    assert!(!log_file.exists(), "fallback log must stay absent");
    Ok(())
}
