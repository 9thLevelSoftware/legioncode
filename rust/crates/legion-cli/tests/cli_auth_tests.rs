//! Integration tests for `legioncode auth pairing list` and `legioncode auth session list`.

mod support;

use std::fs;

use legion_persistence::paths::ServerDerivedPaths;
use legion_persistence::sqlite::SqliteStore;
use serde_json::Value;

use support::{TestResult, isolated_env, run_legioncode};

#[test]
fn auth_pairing_and_session_list_handle_empty_and_active_rows() -> TestResult {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let base_dir = temp.path().join("legionauth");
    fs::create_dir_all(&home)?;
    fs::create_dir_all(&base_dir)?;

    let forward_base = base_dir.to_string_lossy().replace('\\', "/");
    let env = isolated_env(&home, &[]);

    assert_eq!(
        String::from_utf8(
            run_legioncode(
                &["auth", "pairing", "list", "--base-dir", &forward_base],
                &env
            )?
            .stdout
        )?,
        "No active pairing credentials.\n\n"
    );
    assert_eq!(
        String::from_utf8(
            run_legioncode(
                &[
                    "auth",
                    "pairing",
                    "list",
                    "--base-dir",
                    &forward_base,
                    "--json"
                ],
                &env
            )?
            .stdout
        )?,
        "[]\n\n"
    );
    assert_eq!(
        String::from_utf8(
            run_legioncode(
                &["auth", "session", "list", "--base-dir", &forward_base],
                &env
            )?
            .stdout
        )?,
        "No active sessions.\n\n"
    );
    assert_eq!(
        String::from_utf8(
            run_legioncode(
                &[
                    "auth",
                    "session",
                    "list",
                    "--base-dir",
                    &forward_base,
                    "--json"
                ],
                &env
            )?
            .stdout
        )?,
        "[]\n\n"
    );

    seed_auth_rows(&ServerDerivedPaths::derive(&base_dir, false, true))?;
    verify_populated_auth_lists(&forward_base, &env)
}

fn seed_auth_rows(paths: &ServerDerivedPaths) -> TestResult {
    let store = SqliteStore::open(&paths.db_path)?;
    store.connection().execute_batch(
        "INSERT INTO auth_pairing_links (id, credential, method, scopes, subject, label, created_at, expires_at, consumed_at, revoked_at) VALUES
           ('pair-active', 'tok-1', 'one-time-token', '[\"owner\"]', 'cli-user', 'Laptop', '2026-01-01T00:00:00.000Z', '2099-01-01T00:00:00.000Z', NULL, NULL),
           ('pair-boot', 'tok-2', 'desktop-bootstrap', '[\"owner\"]', 'internal-administrative-bootstrap', NULL, '2026-01-01T00:00:00.000Z', '2099-01-01T00:00:00.000Z', NULL, NULL),
           ('pair-used', 'tok-3', 'one-time-token', '[\"owner\"]', 'cli-user', NULL, '2026-01-01T00:00:00.000Z', '2099-01-01T00:00:00.000Z', '2026-01-02T00:00:00.000Z', NULL),
           ('pair-revoked', 'tok-4', 'one-time-token', '[\"owner\"]', 'cli-user', NULL, '2026-01-01T00:00:00.000Z', '2099-01-01T00:00:00.000Z', NULL, '2026-01-02T00:00:00.000Z'),
           ('pair-expired', 'tok-5', 'one-time-token', '[\"owner\"]', 'cli-user', NULL, '2025-01-01T00:00:00.000Z', '2025-02-01T00:00:00.000Z', NULL, NULL);
         INSERT INTO auth_sessions (session_id, subject, scopes, method, client_label, client_device_type, issued_at, expires_at, last_connected_at, revoked_at) VALUES
           ('sess-active', 'remote-user', '[\"orchestration.read\",\"orchestration.write\"]', 'bearer-token', 'MacBook', 'desktop', '2026-01-01T00:00:00.000Z', '2099-01-01T00:00:00.000Z', NULL, NULL),
           ('sess-revoked', 'remote-user', '[\"owner\"]', 'bearer-token', NULL, 'unknown', '2026-01-01T00:00:00.000Z', '2099-01-01T00:00:00.000Z', NULL, '2026-01-02T00:00:00.000Z'),
           ('sess-expired', 'remote-user', '[\"owner\"]', 'bearer-token', NULL, 'unknown', '2025-01-01T00:00:00.000Z', '2025-02-01T00:00:00.000Z', NULL, NULL);",
    )?;
    Ok(())
}

fn verify_populated_auth_lists(forward_base: &str, env: &[(&str, &str)]) -> TestResult {
    let pair_text = String::from_utf8(
        run_legioncode(
            &["auth", "pairing", "list", "--base-dir", forward_base],
            env,
        )?
        .stdout,
    )?;
    assert_eq!(
        pair_text,
        "pair-active (Laptop)\n  scopes: owner\n  created: 2026-01-01T00:00:00.000Z\n  expires: 2099-01-01T00:00:00.000Z\n\n"
    );

    let pair_json_raw = String::from_utf8(
        run_legioncode(
            &[
                "auth",
                "pairing",
                "list",
                "--base-dir",
                forward_base,
                "--json",
            ],
            env,
        )?
        .stdout,
    )?;
    assert!(pair_json_raw.ends_with("\n\n"));
    let pair_json: Vec<Value> = serde_json::from_str(&pair_json_raw)?;
    assert_eq!(pair_json.len(), 1);
    assert_eq!(pair_json[0]["id"], "pair-active");

    let sess_text = String::from_utf8(
        run_legioncode(
            &["auth", "session", "list", "--base-dir", forward_base],
            env,
        )?
        .stdout,
    )?;
    assert_eq!(
        sess_text,
        "sess-active\n  scopes: orchestration.read orchestration.write\n  method: bearer-token\n  subject: remote-user\n  client: MacBook | desktop\n  issued: 2026-01-01T00:00:00.000Z\n  last connected: never\n  expires: 2099-01-01T00:00:00.000Z\n\n"
    );

    let sess_json_raw = String::from_utf8(
        run_legioncode(
            &[
                "auth",
                "session",
                "list",
                "--base-dir",
                forward_base,
                "--json",
            ],
            env,
        )?
        .stdout,
    )?;
    assert!(sess_json_raw.ends_with("\n\n"));
    let sess_json: Vec<Value> = serde_json::from_str(&sess_json_raw)?;
    assert_eq!(sess_json.len(), 1);
    assert_eq!(sess_json[0]["sessionId"], "sess-active");
    Ok(())
}
