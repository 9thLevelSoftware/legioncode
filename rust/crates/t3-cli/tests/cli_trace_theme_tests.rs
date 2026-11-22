//! Integration tests for `t3 trace summary` and `t3 theme show|set|clear` plus Windows path parity.

mod support;

use std::fs;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use t3_persistence::paths::ServerDerivedPaths;

use support::{TestResult, isolated_env, run_t3};

#[test]
fn trace_summary_normalizes_paths_and_aggregates_rotated_spans() -> TestResult {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let base_dir = temp.path().join("t3 home");
    fs::create_dir_all(&home)?;
    fs::create_dir_all(&base_dir)?;

    let forward_base = base_dir.to_string_lossy().replace('\\', "/");
    let env = isolated_env(&home, &[]);

    let out_empty = run_t3(&["trace", "summary", "--base-dir", &forward_base], &env)?;
    assert!(out_empty.status.success());
    let empty_stdout = String::from_utf8(out_empty.stdout)?;
    assert!(empty_stdout.ends_with('\n'));
    assert!(!empty_stdout.ends_with("\n\n"));

    let expected_trace_path = ServerDerivedPaths::derive(&base_dir, false, true)
        .server_trace_path
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        empty_stdout,
        format!("No spans found in {expected_trace_path} or its rotated files.\n")
    );
    if cfg!(windows) {
        assert!(!empty_stdout.contains('/'));
    }

    let env_t3home = isolated_env(&home, &[("T3CODE_HOME", &forward_base)]);
    let out_t3home = run_t3(&["trace", "summary"], &env_t3home)?;
    assert!(out_t3home.status.success());
    if let Ok(node_out) = Command::new("node")
        .args([
            t3_cli::fallback::LEGACY_BIN_TS,
            "trace",
            "summary",
            "--base-dir",
            &forward_base,
        ])
        .env_remove("T3CODE_TRACE_FILE")
        .env_remove("T3CODE_HOME")
        .output()
        && node_out.status.success()
    {
        assert_eq!(out_t3home.stdout, node_out.stdout);
    }

    verify_rotated_trace_spans(&home, temp.path())
}

fn verify_rotated_trace_spans(home: &std::path::Path, root: &std::path::Path) -> TestResult {
    let now_ms = i64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis())?;
    let recent_ns = i128::from(now_ms - 5_000) * 1_000_000;
    let recent_later_ns = i128::from(now_ms - 1_000) * 1_000_000;
    let old_ns = i128::from(now_ms - 7_200_000) * 1_000_000;

    let logs_dir = root.join("logs");
    fs::create_dir_all(&logs_dir)?;
    let custom_trace = logs_dir.join("custom.ndjson");
    let trace_2 = logs_dir.join("custom.ndjson.2");
    let trace_1 = logs_dir.join("custom.ndjson.1");

    fs::write(
        &trace_2,
        format!(
            "{{\"name\":\"ignored.old.backup\",\"durationMs\":99,\"endTimeUnixNano\":\"{recent_ns}\"}}\n"
        ),
    )?;
    fs::write(
        &trace_1,
        format!(
            "{{\"name\":\"span.alpha\",\"durationMs\":400,\"endTimeUnixNano\":\"{recent_ns}\",\"exit\":{{\"_tag\":\"Success\"}}}}\n\
             {{\"name\":\"span.too_old\",\"durationMs\":50,\"endTimeUnixNano\":\"{old_ns}\"}}\n"
        ),
    )?;
    fs::write(
        &custom_trace,
        format!(
            "not json span line\n\
             {{\"name\":\"span.alpha\",\"durationMs\":1200,\"endTimeUnixNano\":\"{recent_later_ns}\",\"status\":{{\"code\":\"2\"}}}}\n\
             {{\"name\":\"span.beta\",\"durationMs\":120,\"endTimeUnixNano\":\"{recent_later_ns}\",\"attributes\":{{\"effect.fiber.interrupted\":true}}}}\n"
        ),
    )?;

    let trace_str = custom_trace.to_string_lossy().into_owned();
    let env_trace = isolated_env(
        home,
        &[
            ("T3CODE_TRACE_FILE", &trace_str),
            ("T3CODE_TRACE_MAX_FILES", "1"),
        ],
    );
    let out_summary = run_t3(
        &["trace", "summary", "--since", "30m", "--limit", "1"],
        &env_trace,
    )?;
    assert!(out_summary.status.success());
    let summary_stdout = String::from_utf8(out_summary.stdout)?;
    assert!(summary_stdout.ends_with('\n'));
    assert!(!summary_stdout.ends_with("\n\n"));
    assert!(summary_stdout.contains("3 spans ended from "));
    assert!(summary_stdout.contains("Skipped 1 lines that are not spans."));
    assert!(summary_stdout.contains("span.alpha"));
    assert!(!summary_stdout.contains("ignored.old.backup"));
    assert!(!summary_stdout.contains("span.too_old"));
    assert!(summary_stdout.contains("1 more span names. Use --limit to show more."));

    let out_bad_dur = run_t3(&["trace", "summary", "--since", "not-a-dur"], &env_trace)?;
    assert_eq!(out_bad_dur.status.code(), Some(1));
    assert!(String::from_utf8(out_bad_dur.stderr)?.contains("Invalid duration"));

    let out_bad_limit = run_t3(&["trace", "summary", "--limit", "0"], &env_trace)?;
    assert_eq!(out_bad_limit.status.code(), Some(1));
    Ok(())
}

#[test]
fn theme_show_and_builtin_set_clear_match_node_double_newlines() -> TestResult {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let base_dir = temp.path().join("t3theme");
    fs::create_dir_all(&home)?;
    fs::create_dir_all(&base_dir)?;

    let forward_base = base_dir.to_string_lossy().replace('\\', "/");
    let env = isolated_env(&home, &[]);
    let paths = ServerDerivedPaths::derive(&base_dir, false, true);

    let out_show_empty = run_t3(&["theme", "show", "--base-dir", &forward_base], &env)?;
    assert!(out_show_empty.status.success());
    let show_stdout = String::from_utf8(out_show_empty.stdout)?;
    assert_eq!(
        show_stdout,
        format!(
            "Environment theme: not set.\n\nPublished themes: none (publish into {}).\n\n",
            paths.environment_themes_dir.display()
        )
    );
    if cfg!(windows) {
        assert!(!show_stdout.contains('/'));
    }

    if let Ok(node_show) = Command::new("node")
        .args([
            t3_cli::fallback::LEGACY_BIN_TS,
            "theme",
            "show",
            "--base-dir",
            &forward_base,
        ])
        .env_remove("T3CODE_HOME")
        .output()
        && node_show.status.success()
    {
        assert_eq!(show_stdout, String::from_utf8(node_show.stdout)?);
    }

    let out_builtin = run_t3(
        &["theme", "set", "grove", "--base-dir", &forward_base],
        &env,
    )?;
    assert!(out_builtin.status.success());
    assert_eq!(
        String::from_utf8(out_builtin.stdout)?,
        format!(
            "Environment theme set to \"grove\" in {}.\n\n",
            paths.settings_path.display()
        )
    );

    let out_clear = run_t3(&["theme", "clear", "--base-dir", &forward_base], &env)?;
    assert!(out_clear.status.success());
    assert_eq!(
        String::from_utf8(out_clear.stdout)?,
        format!(
            "Environment theme cleared in {}.\n\n",
            paths.settings_path.display()
        )
    );
    let settings_json: Value = serde_json::from_str(&fs::read_to_string(&paths.settings_path)?)?;
    assert!(settings_json.get("defaultTheme").is_none());
    assert!(settings_json.get("defaultThemeSetAt").is_none());
    Ok(())
}

#[test]
fn theme_set_publishes_tilde_file_on_windows_and_rejects_bad_targets() -> TestResult {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let base_dir = temp.path().join("t3theme-pub");
    fs::create_dir_all(&home)?;
    fs::create_dir_all(&base_dir)?;
    let base_str = base_dir.to_string_lossy().into_owned();
    let env = isolated_env(&home, &[]);

    let home_theme_dir = home.join("sub");
    fs::create_dir_all(&home_theme_dir)?;
    let theme_file = home_theme_dir.join("mytheme.json");
    fs::write(
        &theme_file,
        r##"{"version":1,"name":"Nord Night","appearance":"dark","canvas":"#1e222a","accent":"#88c0d0"}"##,
    )?;

    let out_publish = run_t3(
        &[
            "theme",
            "set",
            "~/sub/mytheme.json",
            "--id",
            "nord-night",
            "--base-dir",
            &base_str,
        ],
        &env,
    )?;
    assert!(out_publish.status.success());
    assert_eq!(
        String::from_utf8(out_publish.stdout)?,
        format!(
            "Published {} as \"nord-night\" and set it as the environment theme.\n\n",
            theme_file.display()
        )
    );
    assert!(
        base_dir
            .join("userdata")
            .join("themes")
            .join("nord-night.json")
            .is_file()
    );

    let out_show_after = run_t3(&["theme", "show", "--base-dir", &base_str], &env)?;
    assert!(out_show_after.status.success());
    assert_eq!(
        String::from_utf8(out_show_after.stdout)?,
        "Environment theme: \"nord-night\".\n\nPublished themes: nord-night.\n\n"
    );

    assert_eq!(
        run_t3(&["theme", "set", "system", "--base-dir", &base_str], &env)?
            .status
            .code(),
        Some(1)
    );
    assert_eq!(
        run_t3(
            &["theme", "set", "~/missing.json", "--base-dir", &base_str],
            &env
        )?
        .status
        .code(),
        Some(1)
    );
    Ok(())
}
