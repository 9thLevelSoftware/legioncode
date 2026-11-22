//! Integration tests for `legioncode environment list|unlink` and Wave E kill switches.

mod support;

use std::fs;

use legion_contracts::ids::EnvironmentId;
use legion_contracts::orchestration::McpClientAccess;
use legion_persistence::paths::ServerDerivedPaths;
use legion_persistence::sqlite::SqliteStore;
use legion_remote::peer::links::{InsertPeerLinkInput, PeerLinksService};
use legion_remote::secrets::FileSecretStore;
use serde_json::Value;

use support::{TestResult, isolated_env, run_legioncode};

#[test]
fn environment_list_and_unlink_remove_db_row_and_secret_file() -> TestResult {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let base_dir = temp.path().join("legionenv");
    fs::create_dir_all(&home)?;
    fs::create_dir_all(&base_dir)?;

    let forward_base = base_dir.to_string_lossy().replace('\\', "/");
    let env = isolated_env(&home, &[]);

    assert_eq!(
        String::from_utf8(
            run_legioncode(&["environment", "list", "--base-dir", &forward_base], &env)?.stdout
        )?,
        "No linked environments.\n\n"
    );
    assert_eq!(
        String::from_utf8(
            run_legioncode(
                &["environment", "list", "--base-dir", &forward_base, "--json"],
                &env
            )?
            .stdout
        )?,
        "[]\n\n"
    );
    assert_eq!(
        String::from_utf8(
            run_legioncode(
                &[
                    "environment",
                    "unlink",
                    "no-such-env",
                    "--base-dir",
                    &forward_base
                ],
                &env
            )?
            .stdout
        )?,
        "No linked environment no-such-env.\n\n"
    );

    let paths = ServerDerivedPaths::derive(&base_dir, false, true);
    let service = seed_peer_links(&paths)?;
    assert_eq!(fs::read_dir(&paths.secrets_dir)?.count(), 2);

    let out_text = run_legioncode(&["environment", "list", "--base-dir", &forward_base], &env)?;
    assert!(out_text.status.success());
    let text_stdout = String::from_utf8(out_text.stdout)?;
    assert!(text_stdout.ends_with("\n\n"));
    assert!(text_stdout.contains(
        "Alpha Host (peer-active)\n  unreachable, access auto, expires 2099-01-01T00:00:00.000Z\n  https://alpha.tail.ts.net"
    ));
    assert!(text_stdout.contains(
        "Beta Host (peer-expired)\n  expired, access read-only, expires 2025-02-01T00:00:00.000Z\n  https://beta.tail.ts.net"
    ));

    let out_json = run_legioncode(
        &["environment", "list", "--base-dir", &forward_base, "--json"],
        &env,
    )?;
    assert!(out_json.status.success());
    let json_stdout = String::from_utf8(out_json.stdout)?;
    assert!(json_stdout.ends_with("\n\n"));
    let parsed: Vec<Value> = serde_json::from_str(&json_stdout)?;
    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0]["status"], "unreachable");
    assert_eq!(parsed[1]["status"], "expired");

    let out_unlink = run_legioncode(
        &[
            "environment",
            "unlink",
            "peer-active",
            "--base-dir",
            &forward_base,
        ],
        &env,
    )?;
    assert!(out_unlink.status.success());
    assert_eq!(
        String::from_utf8(out_unlink.stdout)?,
        "Forgot peer-active. Revoke it in that environment's Sharing & access to end its session there too.\n\n"
    );

    let store_after = SqliteStore::open(&paths.db_path)?;
    assert!(
        service
            .get(store_after.connection(), "peer-active")?
            .is_none()
    );
    assert!(
        service
            .get(store_after.connection(), "peer-expired")?
            .is_some()
    );
    assert_eq!(fs::read_dir(&paths.secrets_dir)?.count(), 1);
    Ok(())
}

fn seed_peer_links(
    paths: &ServerDerivedPaths,
) -> Result<PeerLinksService, Box<dyn std::error::Error>> {
    let store = SqliteStore::open(&paths.db_path)?;
    let secrets = FileSecretStore::new(&paths.secrets_dir)?;
    let service = PeerLinksService::new(secrets);

    service.insert(
        store.connection(),
        InsertPeerLinkInput {
            environment_id: EnvironmentId::new("peer-active")?,
            label: "Alpha Host".to_owned(),
            urls: vec!["https://alpha.tail.ts.net/".to_owned()],
            access: McpClientAccess::Auto,
            access_token: "secret-alpha".to_owned(),
            linked_at: "2026-01-01T00:00:00.000Z".to_owned(),
            expires_at: "2099-01-01T00:00:00.000Z".to_owned(),
        },
    )?;
    service.insert(
        store.connection(),
        InsertPeerLinkInput {
            environment_id: EnvironmentId::new("peer-expired")?,
            label: "Beta Host".to_owned(),
            urls: vec!["https://beta.tail.ts.net/".to_owned()],
            access: McpClientAccess::ReadOnly,
            access_token: "secret-beta".to_owned(),
            linked_at: "2025-01-01T00:00:00.000Z".to_owned(),
            expires_at: "2025-02-01T00:00:00.000Z".to_owned(),
        },
    )?;
    Ok(service)
}

#[test]
fn wave_e_kill_switches_and_unsupported_subcommands_delegate_to_node() -> TestResult {
    let temp = tempfile::tempdir()?;
    let log_path = temp.path().join("fallback.jsonl");
    let log_str = log_path.to_string_lossy().into_owned();
    let empty_bin = temp.path().join("empty-bin");
    fs::create_dir_all(&empty_bin)?;
    let empty_bin_str = empty_bin.to_string_lossy().into_owned();

    let forced_env = isolated_env(
        temp.path(),
        &[
            ("RUST_ANYTHING_FALLBACK_LOG", &log_str),
            ("PATH", &empty_bin_str),
            ("RUST_ANYTHING_UNIT_TRACE", "original"),
            ("RUST_ANYTHING_UNIT_THEME", "ORIGINAL"),
            ("RUST_ANYTHING_UNIT_ENVIRONMENT", "OrIgInAl"),
            ("RUST_ANYTHING_UNIT_AUTH", "original"),
        ],
    );

    assert_eq!(
        run_legioncode(&["trace", "summary"], &forced_env)?
            .status
            .code(),
        Some(1)
    );
    assert_eq!(
        run_legioncode(&["theme", "show"], &forced_env)?
            .status
            .code(),
        Some(1)
    );
    assert_eq!(
        run_legioncode(&["environment", "list"], &forced_env)?
            .status
            .code(),
        Some(1)
    );
    assert_eq!(
        run_legioncode(&["auth", "pairing", "list"], &forced_env)?
            .status
            .code(),
        Some(1)
    );

    let subcmd_env = isolated_env(
        temp.path(),
        &[
            ("RUST_ANYTHING_FALLBACK_LOG", &log_str),
            ("PATH", &empty_bin_str),
            ("LEGIONCODE_BIN_TS", r"Z:\nonexistent\bin.ts"),
        ],
    );
    assert_eq!(
        run_legioncode(
            &[
                "environment",
                "link",
                "http://127.0.0.1:9",
                "--pairing-code",
                "abc"
            ],
            &subcmd_env
        )?
        .status
        .code(),
        Some(1)
    );
    assert_eq!(
        run_legioncode(&["auth", "pairing", "create"], &subcmd_env)?
            .status
            .code(),
        Some(0)
    );

    let entries: Vec<Value> = fs::read_to_string(&log_path)?
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(entries.len(), 4);
    assert_eq!(entries[0]["unit"], "TRACE");
    assert_eq!(entries[1]["unit"], "THEME");
    assert_eq!(entries[2]["unit"], "ENVIRONMENT");
    assert_eq!(entries[3]["unit"], "AUTH");
    Ok(())
}
