//! Integration tests for streaming trace diagnostics and guarded environment theme publishing.

use std::fs;
use std::path::Path;

use legion_remote::theme::{
    BUILT_IN_THEME_IDS, CONCURRENT_WRITE_ATTEMPTS, MAX_THEME_FILE_BYTES, RESERVED_THEME_IDS,
    ThemeError, UNPUBLISHABLE_THEME_IDS, clear_default_theme, format_theme_cleared,
    format_theme_published_and_set, format_theme_set, format_theme_show, is_valid_theme_id,
    publish_theme_file, read_and_validate_theme_file, read_default_theme, read_published_themes,
    read_theme_file_guarded, set_default_theme, set_default_theme_with_hook,
};
use legion_remote::trace::{
    DEFAULT_TRACE_MAX_FILES, TraceSpanSummaryBuilder, format_duration_ms,
    format_empty_trace_message, format_trace_summary, summarize_trace_files,
    to_rotated_trace_paths,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn trace_rotated_paths_and_streaming_summary() -> TestResult {
    assert_eq!(DEFAULT_TRACE_MAX_FILES, 5);
    let dir = tempfile::tempdir()?;
    let active = dir.path().join("trace.ndjson");
    let rotated = to_rotated_trace_paths(&active, 3);
    assert_eq!(
        rotated,
        vec![
            dir.path().join("trace.ndjson.3"),
            dir.path().join("trace.ndjson.2"),
            dir.path().join("trace.ndjson.1"),
            active.clone(),
        ]
    );

    // Rotated .1 has one old pre-window span and one valid fast span; active has OTLP status failure + attr interrupt + invalid ns.
    fs::write(
        dir.path().join("trace.ndjson.1"),
        "\n   \n\
         {\"name\":\"db.query\",\"durationMs\":50,\"endTimeUnixNano\":\"1700000000000000000\"}\n\
         {\"name\":\"db.query\",\"durationMs\":100,\"endTimeUnixNano\":\"1700000010000000000\"}\n",
    )?;
    let bad_ns = "9".repeat(22);
    fs::write(
        &active,
        format!(
            "{}\n{}\n{{\"name\":\"bad.ts\",\"durationMs\":1,\"endTimeUnixNano\":\"{bad_ns}\"}}\n{{not json\n",
            r#"{"name":"db.query","durationMs":1250,"endTimeUnixNano":"1700000070000000000","status":{"code":"2","message":"timeout"}}"#,
            r#"{"name":"rpc.call","durationMs":999.6,"endTimeUnixNano":"1700000070000000000","attributes":{"effect.fiber.interrupted":true}}"#,
        ),
    )?;

    let summary = summarize_trace_files(&active, DEFAULT_TRACE_MAX_FILES, Some(1_700_000_005_000))?;
    assert_eq!(summary.span_count, 3);
    assert_eq!(summary.skipped_line_count, 2);
    assert_eq!(summary.spans[0].name, "db.query");
    assert_eq!(summary.spans[0].count, 2);
    assert_eq!(summary.spans[0].p50_ms, 100.0);
    assert_eq!(summary.spans[0].p90_ms, 1250.0);
    assert_eq!(summary.spans[0].max_ms, 1250.0);
    assert_eq!(summary.spans[0].failure_count, 1);
    assert_eq!(summary.spans[1].name, "rpc.call");
    assert_eq!(summary.spans[1].interrupted_count, 1);
    Ok(())
}

#[test]
fn trace_cli_formatting_byte_exact() {
    assert_eq!(format_duration_ms(999.4), "999ms");
    assert_eq!(format_duration_ms(999.6), "1s");
    assert_eq!(format_duration_ms(1250.0), "1.3s");

    let mut builder = TraceSpanSummaryBuilder::new(None);
    // 1_000 minutes window -> 1 span gives 0.001/min => "<0.1"
    builder.ingest_line(
        r#"{"name":"rare.op","durationMs":999.6,"endTimeUnixNano":"1700000000000000000"}"#,
    );
    builder.ingest_line(
        r#"{"name":"common.op","durationMs":12,"endTimeUnixNano":"1700060000000000000"}"#,
    );
    builder.ingest_line(
        r#"{"name":"common.op","durationMs":20,"endTimeUnixNano":"1700060000000000000"}"#,
    );
    let summary = builder.build();
    let rendered = format_trace_summary(&summary, 1);
    assert!(!rendered.ends_with('\n'));
    assert!(rendered.contains(
        "3 spans ended from 2023-11-14T22:13:20.000Z to 2023-11-15T14:53:20.000Z (1000 min)."
    ));
    assert!(rendered.contains("1 more span names. Use --limit to show more."));

    let full = format_trace_summary(&summary, 10);
    assert!(full.contains("common.op"));
    assert!(full.contains("rare.op"));
    assert!(full.contains("<0.1"));

    assert_eq!(
        format_empty_trace_message(Path::new("/tmp/trace.ndjson"), true, 1),
        "No spans found in /tmp/trace.ndjson or its rotated files in that window. Skipped 1 lines that are not spans."
    );
}

#[test]
fn theme_validation_constants_and_guarded_reader() -> TestResult {
    assert_eq!(
        BUILT_IN_THEME_IDS,
        ["legion-chat", "grove", "ocean", "ember", "iris"]
    );
    assert_eq!(RESERVED_THEME_IDS.len(), 8);
    assert_eq!(UNPUBLISHABLE_THEME_IDS.len(), 9);
    assert!(UNPUBLISHABLE_THEME_IDS.contains(&"legion-code"));
    assert_eq!(MAX_THEME_FILE_BYTES, 616_384);

    assert!(is_valid_theme_id("nord-1"));
    assert!(!is_valid_theme_id("system"));
    assert!(!is_valid_theme_id("light"));
    assert!(!is_valid_theme_id("dark"));
    assert!(!is_valid_theme_id("-leading-dash"));

    let dir = tempfile::tempdir()?;
    // Guarded reader rejects directories (non-regular) and oversize files, strips UTF-8 BOM.
    let sub_dir = dir.path().join("dir-not-file");
    fs::create_dir_all(&sub_dir)?;
    assert!(matches!(
        read_theme_file_guarded(&sub_dir, MAX_THEME_FILE_BYTES),
        Err(ThemeError::FileNotRegular { .. })
    ));

    #[cfg(unix)]
    {
        let target = dir.path().join("real.json");
        fs::write(&target, b"{}")?;
        let link = dir.path().join("sym.json");
        std::os::unix::fs::symlink(&target, &link)?;
        assert!(matches!(
            read_theme_file_guarded(&link, MAX_THEME_FILE_BYTES),
            Err(ThemeError::FileSymlink { .. })
        ));
    }

    let bom_file = dir.path().join("nord.json");
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice(
        br##"{"version":1,"name":"Nord","appearance":"dark","canvas":"#111","accent":"#abc"}"##,
    );
    fs::write(&bom_file, &bytes)?;
    let (_, parsed) = read_and_validate_theme_file(&bom_file)?;
    assert_eq!(parsed.name, "Nord");

    let colorless = dir.path().join("colorless.json");
    fs::write(&colorless, br#"{"name":"Blank","appearance":"light"}"#)?;
    assert!(matches!(
        read_and_validate_theme_file(&colorless),
        Err(ThemeError::FileColorless { .. })
    ));

    let themes_dir = dir.path().join("themes");
    fs::create_dir_all(&themes_dir)?;
    fs::write(
        themes_dir.join("nord.json"),
        br##"{"name":"Nord","appearance":"dark","colors":{"bgPrimary":"#111111"}}"##,
    )?;
    fs::write(
        themes_dir.join("system.json"),
        br##"{"name":"Reserved","appearance":"dark","colors":{"a":"#000"}}"##,
    )?;
    let loaded = read_published_themes(&themes_dir);
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].id, "nord");
    Ok(())
}

#[test]
fn theme_publish_rollback_cas_settings_and_formatters() -> TestResult {
    assert_eq!(CONCURRENT_WRITE_ATTEMPTS, 5);
    let dir = tempfile::tempdir()?;
    let themes_dir = dir.path().join("themes");
    let settings_path = dir.path().join("settings.json");

    // Atomic publish preserves rollback and cleans up safely.
    let v1 = br##"{"name":"Solar","appearance":"light","canvas":"#ffffff","accent":"#ff8800"}"##;
    let h1 = publish_theme_file(&themes_dir, "solar", v1)?;
    h1.cleanup()?;
    assert_eq!(fs::read(themes_dir.join("solar.json"))?, v1);

    // Reverting restores v1 after failed settings update.
    let v2 = br##"{"name":"Solar Two","appearance":"dark","canvas":"#000000","accent":"#ff8800"}"##;
    let h2 = publish_theme_file(&themes_dir, "solar", v2)?;
    assert_eq!(fs::read(themes_dir.join("solar.json"))?, v2);
    h2.revert()?;
    assert_eq!(fs::read(themes_dir.join("solar.json"))?, v1);

    // Lenient JSONC settings preserve existing keys and update/clear defaultTheme & defaultThemeSetAt.
    fs::write(
        &settings_path,
        "{\n  // user comment\n  \"customFlag\": true,\n}\n",
    )?;
    set_default_theme(&settings_path, "solar", 1_700_000_000_000)?;
    assert_eq!(
        read_default_theme(&settings_path)?.as_deref(),
        Some("solar")
    );
    let saved = fs::read_to_string(&settings_path)?;
    assert!(saved.contains("\"customFlag\": true"));
    assert!(saved.contains("\"defaultThemeSetAt\": \"2023-11-14T22:13:20.000Z\""));

    // Simulate concurrent modification across all 5 CAS attempts -> SettingsBusy.
    let busy = set_default_theme_with_hook(&settings_path, "other", 1_700_000_000_500, |attempt| {
        let _ = fs::write(
            &settings_path,
            format!("{{\"customFlag\": true, \"race\": {attempt}}}\n"),
        );
    });
    assert!(matches!(
        busy,
        Err(ThemeError::SettingsBusy { attempts: 5, .. })
    ));

    clear_default_theme(&settings_path)?;
    assert_eq!(read_default_theme(&settings_path)?, None);
    let cleared = fs::read_to_string(&settings_path)?;
    assert!(cleared.contains("\"customFlag\": true"));
    assert!(!cleared.contains("defaultTheme"));

    assert_eq!(
        format_theme_published_and_set(Path::new("/tmp/a.json"), "solar"),
        "Published /tmp/a.json as \"solar\" and set it as the environment theme.\n"
    );
    assert_eq!(
        format_theme_set("solar", Path::new("/tmp/settings.json")),
        "Environment theme set to \"solar\" in /tmp/settings.json.\n"
    );
    assert_eq!(
        format_theme_cleared(Path::new("/tmp/settings.json")),
        "Environment theme cleared in /tmp/settings.json.\n"
    );
    assert_eq!(
        format_theme_show(None, &[], Path::new("/tmp/themes")),
        "Environment theme: not set.\nPublished themes: none (publish into /tmp/themes).\n"
    );
    assert_eq!(
        format_theme_show(
            Some("solar"),
            &["zeta".to_owned(), "alpha".to_owned()],
            Path::new("/tmp/themes")
        ),
        "Environment theme: \"solar\".\nPublished themes: alpha, zeta.\n"
    );
    Ok(())
}

#[test]
fn theme_publish_directory_collision_and_concurrent_revert_safety() -> TestResult {
    let dir = tempfile::tempdir()?;
    let themes_dir = dir.path().join("themes");
    let colliding = themes_dir.join("dir-theme.json");
    fs::create_dir_all(&colliding)?;

    let err = publish_theme_file(
        &themes_dir,
        "dir-theme",
        br#"{"name":"X","appearance":"dark"}"#,
    );
    assert!(matches!(err, Err(ThemeError::DestinationDirectory { .. })));

    // If a concurrent publish replaces a newly published theme before settings rollback runs,
    // handle.revert() must NOT clobber the newer file.
    let v1 = br##"{"name":"Alpha","appearance":"dark","canvas":"#111","accent":"#222"}"##;
    let h1 = publish_theme_file(&themes_dir, "alpha-safe", v1)?;
    let v2 = br##"{"name":"Alpha Newer Much Longer Name","appearance":"light","canvas":"#eee","accent":"#333"}"##;
    fs::write(themes_dir.join("alpha-safe.json"), v2)?;
    h1.revert()?;
    assert_eq!(fs::read(themes_dir.join("alpha-safe.json"))?, v2);
    Ok(())
}
