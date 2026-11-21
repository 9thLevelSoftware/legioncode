//! Static `delegate_to_node` audit plus zero-Node tests for global flags (`--completions`,
//! `--log-level`, `--wizard`), subcommand `--help`/`-h`, bare `t3`, and Effect CLI errors.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt as _, BufReader};
use tokio::process::Command;

use crate::support::{TestResult, isolated_env, run_t3};

const NONEXISTENT_BIN_TS: &str = r"Z:\nonexistent\bin.ts";

fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_rs_files(&path, out)?;
        } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            out.push(path);
        }
    }
    Ok(())
}

#[test]
fn static_audit_src_files_under_350_loc_and_zero_implicit_node_fallback() -> TestResult {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let src_dir = manifest_dir.join("src");
    let mut files = Vec::new();
    collect_rs_files(&src_dir, &mut files)?;
    files.sort();
    assert_ne!(files, [] as [PathBuf; 0]);

    for file in &files {
        let rel = file
            .strip_prefix(&src_dir)?
            .to_string_lossy()
            .replace('\\', "/");
        let text = fs::read_to_string(file)?;
        let lines: Vec<&str> = text.lines().collect();
        assert!(
            lines.len() <= 350,
            "src/{rel} has {} lines (must be <= 350)",
            lines.len()
        );
        assert!(
            !text.contains(concat!("todo", "!(")) && !text.contains(concat!("unimplemented", "!(")),
            "src/{rel} contains unfinished stub macro"
        );
        if rel == "fallback.rs" {
            continue;
        }
        for (idx, line) in lines.iter().enumerate() {
            if line.contains("delegate_to_node(") {
                let start = idx.saturating_sub(3);
                let window = lines[start..=idx].join("\n");
                assert!(
                    window.contains("unit_forced_original("),
                    "src/{rel}:{} calls delegate_to_node outside unit_forced_original guard:\n{window}",
                    idx + 1
                );
            }
            if line.contains("CLI_PASSTHROUGH") {
                assert_eq!(
                    rel, "lib.rs",
                    "CLI_PASSTHROUGH unexpectedly referenced in src/{rel}"
                );
            }
        }
    }
    Ok(())
}

#[test]
fn global_flags_completions_wizard_log_level_and_errors_without_node() -> TestResult {
    let temp = tempfile::tempdir()?;
    let empty_bin = temp.path().join("empty-bin");
    fs::create_dir_all(&empty_bin)?;
    let empty_str = empty_bin.to_string_lossy().into_owned();
    let log_file = temp.path().join("global-fallback.jsonl");
    let log_str = log_file.to_string_lossy().into_owned();
    let env = isolated_env(
        temp.path(),
        &[
            ("PATH", &empty_str),
            ("T3_BIN_TS", NONEXISTENT_BIN_TS),
            ("RUST_ANYTHING_FALLBACK_LOG", &log_str),
        ],
    );

    // 1. All 6 shells + inline `--completions=<shell>`.
    for (shell, marker) in [
        ("bash", "_init_completion"),
        ("sh", "_init_completion"),
        ("zsh", "#compdef t3"),
        ("fish", "__fish_use_subcommand"),
        ("powershell", "###-begin-t3-completions-###"),
        ("pwsh", "###-begin-t3-completions-###"),
    ] {
        let out = run_t3(&["--completions", shell], &env)?;
        assert_eq!(out.status.code(), Some(0), "shell={shell}");
        let stdout = String::from_utf8(out.stdout)?;
        assert!(stdout.contains(marker), "shell={shell}");
        assert_eq!(out.stderr, [] as [u8; 0]);
    }
    let inline = run_t3(&["--completions=zsh"], &env)?;
    assert_eq!(inline.status.code(), Some(0));
    assert!(String::from_utf8(inline.stdout)?.contains("#compdef t3"));

    // 2. Missing & invalid `--completions` and `--log-level`.
    let miss_comp = run_t3(&["--completions"], &env)?;
    assert_eq!(miss_comp.status.code(), Some(1));
    assert!(String::from_utf8(miss_comp.stderr)?.contains("Missing value for flag --completions"));

    let bad_comp = run_t3(&["--completions", "tcsh"], &env)?;
    assert_eq!(bad_comp.status.code(), Some(1));
    assert!(
        String::from_utf8(bad_comp.stderr)?
            .contains("Invalid value for flag --completions: \"tcsh\"")
    );

    let bad_lvl = run_t3(&["--log-level", "loud", "--version"], &env)?;
    assert_eq!(bad_lvl.status.code(), Some(1));
    assert!(
        String::from_utf8(bad_lvl.stderr)?.contains("Invalid value for flag --log-level: \"loud\"")
    );

    // 3. Valid `--log-level` strips cleanly before command dispatch.
    let ver = run_t3(&["--log-level", "warning", "--version"], &env)?;
    assert_eq!(ver.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(ver.stdout)?,
        format!("t3 v{}\n", env!("CARGO_PKG_VERSION"))
    );

    // 4. Non-TTY `--wizard` prints cancel banner and exits 0.
    let wiz = run_t3(&["--wizard"], &env)?;
    assert_eq!(wiz.status.code(), Some(0));
    let wiz_out = String::from_utf8(wiz.stdout)?;
    assert!(wiz_out.contains("Command wizard"));
    assert!(wiz_out.contains("Wizard cancelled."));

    let wiz_sub = run_t3(&["status", "--wizard"], &env)?;
    assert_eq!(wiz_sub.status.code(), Some(0));
    assert!(String::from_utf8(wiz_sub.stdout)?.contains("$ t3 status"));

    assert!(!log_file.exists(), "fallback log must stay absent");
    Ok(())
}

#[tokio::test]
async fn subcommand_help_and_bare_root_server_and_error_formatting_zero_node() -> TestResult {
    let temp = tempfile::tempdir()?;
    let empty_bin = temp.path().join("empty-bin");
    fs::create_dir_all(&empty_bin)?;
    let empty_str = empty_bin.to_string_lossy().into_owned();
    let log_file = temp.path().join("help-root-fallback.jsonl");
    let log_str = log_file.to_string_lossy().into_owned();
    let env = isolated_env(
        temp.path(),
        &[
            ("PATH", &empty_str),
            ("T3_BIN_TS", NONEXISTENT_BIN_TS),
            ("RUST_ANYTHING_FALLBACK_LOG", &log_str),
        ],
    );

    // 1. Nested `--help` and `-h` work across core & subcommand trees without Node.
    let help_paths: &[&[&str]] = &[
        &["serve", "--help"],
        &["start", "-h"],
        &["status", "--help"],
        &["doctor", "--help"],
        &["auth", "--help"],
        &["auth", "pairing", "create", "--help"],
        &["environment", "link", "--help"],
        &["pair", "--help"],
        &["project", "add", "--help"],
        &["thread", "start", "--help"],
        &["policy", "templates", "--help"],
        &["service", "protect", "--help"],
    ];
    for args in help_paths {
        let out = run_t3(args, &env)?;
        assert_eq!(out.status.code(), Some(0), "args={args:?}");
        let stdout = String::from_utf8(out.stdout)?;
        assert!(stdout.contains("USAGE"), "args={args:?}");
        assert_eq!(out.stderr, [] as [u8; 0], "args={args:?}");
    }

    let cloud_env = isolated_env(
        temp.path(),
        &[
            ("PATH", &empty_str),
            ("T3_BIN_TS", NONEXISTENT_BIN_TS),
            ("RUST_ANYTHING_FALLBACK_LOG", &log_str),
            ("T3CODE_CLOUD_PUBLIC_CONFIG", "1"),
        ],
    );
    let conn_help = run_t3(&["connect", "status", "--help"], &cloud_env)?;
    assert_eq!(conn_help.status.code(), Some(0));
    assert!(String::from_utf8(conn_help.stdout)?.contains("connect status"));

    // 2. Unknown root flag and bogus bare word emit Effect CLI stderr + exit 1 without Node.
    let bad_flag = run_t3(&["--definitely-unknown-flag"], &env)?;
    assert_eq!(bad_flag.status.code(), Some(1));
    assert!(String::from_utf8(bad_flag.stderr)?.starts_with("\nERROR\n"));

    let bogus_word = run_t3(&["not-an-existing-dir-or-cmd"], &env)?;
    assert_eq!(bogus_word.status.code(), Some(1));
    assert!(
        String::from_utf8(bogus_word.stderr)?
            .contains("Unknown command \"not-an-existing-dir-or-cmd\"")
    );

    // 3. Bare `t3 --port 0 --no-browser` boots live HTTP server natively (`/.well-known/t3/environment`).
    let srv_home = temp.path().join("bare-root-home");
    let srv_home_str = srv_home.to_string_lossy().into_owned();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_t3"));
    cmd.args(["--port", "0", "--no-browser", "--base-dir", &srv_home_str])
        .env("PATH", &empty_str)
        .env("T3_BIN_TS", NONEXISTENT_BIN_TS)
        .env("RUST_ANYTHING_FALLBACK_LOG", &log_str)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    let mut reader = BufReader::new(child.stdout.take().ok_or("missing stdout")?);
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    let origin = line
        .trim()
        .strip_prefix("T3 Code server listening on ")
        .ok_or_else(|| format!("unexpected server banner: {line:?}"))?
        .to_owned();

    let desc_text = reqwest::get(format!("{origin}/.well-known/t3/environment"))
        .await?
        .text()
        .await?;
    let desc: Value = serde_json::from_str(&desc_text)?;
    assert!(
        desc["environmentId"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
    );

    let _ = child.kill().await;
    let _ = child.wait().await;
    assert!(!log_file.exists(), "fallback log must stay absent");
    Ok(())
}
