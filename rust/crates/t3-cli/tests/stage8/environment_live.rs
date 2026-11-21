//! Zero-Node live 2-server `t3 environment link -> list -> unlink` integration test.

use std::fs;

use serde_json::Value;
use t3_cli::serve::{ServeConfig, ServeMode, spawn_server};
use t3_persistence::pairing_tokens::{IssuePairingTokenInput, issue_pairing_token};

use crate::support::{TestResult, isolated_env, run_t3};

const NONEXISTENT_BIN_TS: &str = r"Z:\nonexistent\bin.ts";

fn run_ok(args: &[&str], env: &[(&str, &str)]) -> Result<String, Box<dyn std::error::Error>> {
    let out = run_t3(args, env)?;
    assert_eq!(
        out.status.code(),
        Some(0),
        "args={args:?} stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(String::from_utf8(out.stdout)?)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_two_server_environment_link_list_unlink_zero_node() -> TestResult {
    let temp = tempfile::tempdir()?;
    let empty_bin = temp.path().join("empty-bin");
    fs::create_dir_all(&empty_bin)?;
    let empty_str = empty_bin.to_string_lossy().into_owned();
    let log_file = temp.path().join("env-stage8-fallback.jsonl");
    let log_str = log_file.to_string_lossy().into_owned();
    let env = isolated_env(
        temp.path(),
        &[
            ("PATH", &empty_str),
            ("T3_BIN_TS", NONEXISTENT_BIN_TS),
            ("RUST_ANYTHING_FALLBACK_LOG", &log_str),
        ],
    );

    let peer_home = temp.path().join("peer-home");
    let client_home = temp.path().join("client-home");
    let client_str = client_home.to_string_lossy().into_owned();
    let peer_server = spawn_server(ServeConfig {
        base_dir: peer_home.clone(),
        base_dir_is_explicit: true,
        port: 0,
        host: Some("127.0.0.1".to_owned()),
        mode: ServeMode::Web,
        no_browser: true,
        lan: false,
        static_dir: None,
        cwd: temp.path().to_path_buf(),
    })
    .await?;

    let now_ms = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis(),
    )?;
    let issued = issue_pairing_token(
        &peer_server.paths().db_path,
        &IssuePairingTokenInput::default().with_ttl_ms(120_000),
        now_ms,
    )?;

    let link_out = run_ok(
        &[
            "environment",
            "link",
            peer_server.origin(),
            "--pairing-code",
            &issued.credential,
            "--access",
            "auto",
            "--base-dir",
            &client_str,
        ],
        &env,
    )?;
    assert!(link_out.contains("Linked "));

    let list_out = run_ok(
        &["environment", "list", "--json", "--base-dir", &client_str],
        &env,
    )?;
    let peers: Vec<Value> = serde_json::from_str(&list_out)?;
    assert_eq!(peers.len(), 1);
    let peer_env_id = peers[0]["environmentId"]
        .as_str()
        .ok_or("missing environmentId")?
        .to_owned();
    assert_eq!(peers[0]["access"], "auto");

    let unlink_out = run_ok(
        &[
            "environment",
            "unlink",
            &peer_env_id,
            "--base-dir",
            &client_str,
        ],
        &env,
    )?;
    assert!(unlink_out.contains(&format!("Forgot {peer_env_id}.")));

    verify_pair_tailscale_zero_node(temp.path(), &empty_str, &log_file, &peer_home, &client_str)?;
    peer_server.shutdown().await?;
    Ok(())
}

fn verify_pair_tailscale_zero_node(
    temp_root: &std::path::Path,
    empty_str: &str,
    log_file: &std::path::Path,
    peer_home: &std::path::Path,
    client_str: &str,
) -> TestResult {
    let log_str = log_file.to_string_lossy().into_owned();
    let pair_tailscale = run_t3(
        &[
            "pair",
            "--tailscale",
            "--base-dir",
            &peer_home.to_string_lossy(),
        ],
        &isolated_env(
            temp_root,
            &[
                ("PATH", empty_str),
                ("T3_BIN_TS", NONEXISTENT_BIN_TS),
                ("RUST_ANYTHING_FALLBACK_LOG", &log_str),
                ("T3CODE_TAILSCALE_BIN", r"Z:\nonexistent\tailscale"),
            ],
        ),
    )?;
    assert_eq!(pair_tailscale.status.code(), Some(1));
    assert!(!log_file.exists(), "fallback log must stay absent");

    let ks_env = isolated_env(
        temp_root,
        &[
            ("PATH", empty_str),
            ("RUST_ANYTHING_FALLBACK_LOG", &log_str),
            ("RUST_ANYTHING_UNIT_PAIR", "original"),
        ],
    );
    assert_eq!(
        run_t3(&["pair", "--tailscale", "--base-dir", client_str], &ks_env)?
            .status
            .code(),
        Some(1)
    );
    assert!(fs::read_to_string(log_file)?.contains(r#""unit":"PAIR""#));
    Ok(())
}
