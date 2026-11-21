//! Native `t3 trace summary` implementation matching `apps/server/src/cli/trace.ts`.

use std::env;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use t3_remote::trace::{
    DEFAULT_TRACE_MAX_FILES, format_empty_trace_message, format_trace_summary,
    summarize_trace_files,
};

use crate::cli_global::fail_with_command_help;
use crate::fallback::{
    delegate_to_node, derive_paths, take_flag_value, unit_forced_original, write_stdout_line,
};

/// Unit identifier for `RUST_ANYTHING_UNIT_TRACE` and fallback logs.
pub const UNIT_TRACE: &str = "TRACE";

/// Default maximum number of distinct span names printed by `t3 trace summary`.
pub const DEFAULT_TRACE_LIMIT: usize = 25;

const TRACE_FILE_ENV: &str = "T3CODE_TRACE_FILE";
const TRACE_MAX_FILES_ENV: &str = "T3CODE_TRACE_MAX_FILES";

#[derive(Debug, Clone, PartialEq, Eq)]
struct TraceSummaryFlags {
    base_dir: Option<String>,
    since: Option<String>,
    limit: usize,
}

/// Runs `t3 trace summary` natively unless `RUST_ANYTHING_UNIT_TRACE=original` is set.
pub fn run(args: &[String]) -> i32 {
    if unit_forced_original(UNIT_TRACE) {
        return delegate_to_node(UNIT_TRACE, args);
    }

    let Some(subcmd) = args.get(1).map(String::as_str) else {
        return fail_with_command_help(&["trace"], "Missing subcommand for `t3 trace`.");
    };
    if subcmd != "summary" {
        return fail_with_command_help(
            &["trace"],
            &format!("Unknown subcommand \"{subcmd}\" for \"t3 trace\""),
        );
    }

    let rest = args.get(2..).unwrap_or(&[]);
    let flags = match parse_summary_flags(rest) {
        Ok(Some(flags)) => flags,
        Ok(None) => {
            return fail_with_command_help(
                &["trace", "summary"],
                "Invalid or unrecognized arguments for `t3 trace summary`.",
            );
        }
        Err(msg) => {
            eprintln!("{msg}");
            return 1;
        }
    };

    let since_ms = match flags.since.as_deref() {
        Some(raw) => {
            let Some(dur_ms) = parse_duration_ms(raw) else {
                eprintln!("Invalid duration. Use values like 5m, 1h, 30d, or 15 minutes.");
                return 1;
            };
            Some(current_epoch_ms().saturating_sub(dur_ms))
        }
        None => None,
    };

    let max_files = match resolve_trace_max_files() {
        Ok(n) => n,
        Err(msg) => {
            eprintln!("{msg}");
            return 1;
        }
    };

    let trace_path = resolve_trace_file_path(flags.base_dir.as_deref());
    let summary = match summarize_trace_files(&trace_path, max_files, since_ms) {
        Ok(summary) => summary,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };

    let rendered = if summary.span_count == 0 {
        format_empty_trace_message(&trace_path, since_ms.is_some(), summary.skipped_line_count)
    } else {
        format_trace_summary(&summary, flags.limit)
    };

    write_stdout_line(&rendered, "trace summary")
}

fn parse_summary_flags(rest: &[String]) -> Result<Option<TraceSummaryFlags>, String> {
    let mut base_dir = None;
    let mut since = None;
    let mut limit = DEFAULT_TRACE_LIMIT;
    let mut idx = 0;

    while idx < rest.len() {
        if let Ok(Some(val)) = take_flag_value(rest, &mut idx, "base-dir") {
            base_dir = Some(val);
        } else if let Ok(Some(val)) = take_flag_value(rest, &mut idx, "since") {
            since = Some(val);
        } else if let Ok(Some(val)) = take_flag_value(rest, &mut idx, "limit") {
            let parsed = val
                .trim()
                .parse::<usize>()
                .ok()
                .filter(|n| *n >= 1)
                .ok_or_else(|| {
                    format!("Invalid value `{val}` for --limit: expected a positive integer.")
                })?;
            limit = parsed;
        } else {
            return Ok(None);
        }
        idx += 1;
    }

    Ok(Some(TraceSummaryFlags {
        base_dir,
        since,
        limit,
    }))
}

fn resolve_trace_file_path(base_dir: Option<&str>) -> PathBuf {
    if let Ok(raw) = env::var(TRACE_FILE_ENV) {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    derive_paths(base_dir).1.server_trace_path
}

fn resolve_trace_max_files() -> Result<usize, String> {
    let Ok(raw) = env::var(TRACE_MAX_FILES_ENV) else {
        return Ok(DEFAULT_TRACE_MAX_FILES);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(DEFAULT_TRACE_MAX_FILES);
    }
    trimmed
        .parse::<usize>()
        .map_err(|_| format!("Invalid {TRACE_MAX_FILES_ENV} value `{trimmed}`."))
}

/// Parses a human CLI duration expression (`30m`, `2h`, `45s`, `1d`, `1w`, `500ms`, or `"15 minutes"`)
/// into whole milliseconds.
///
/// ```
/// use t3_cli::cmd_trace::parse_duration_ms;
///
/// assert_eq!(parse_duration_ms("45s"), Some(45_000));
/// assert_eq!(parse_duration_ms("30m"), Some(1_800_000));
/// assert_eq!(parse_duration_ms("2h"), Some(7_200_000));
/// assert_eq!(parse_duration_ms("1d"), Some(86_400_000));
/// assert_eq!(parse_duration_ms("15 minutes"), Some(900_000));
/// assert_eq!(parse_duration_ms("bad"), None);
/// ```
#[must_use]
pub fn parse_duration_ms(raw: &str) -> Option<i64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let digit_end = trimmed
        .find(|ch: char| !ch.is_ascii_digit())
        .unwrap_or(trimmed.len());
    if digit_end == 0 {
        return None;
    }
    let count = trimmed[..digit_end].parse::<i64>().ok()?;
    let unit = trimmed[digit_end..].trim().to_ascii_lowercase();
    let unit_ms: i64 = match unit.as_str() {
        "ms" | "milli" | "millis" | "millisecond" | "milliseconds" => 1,
        "s" | "sec" | "secs" | "second" | "seconds" => 1_000,
        "m" | "min" | "mins" | "minute" | "minutes" => 60_000,
        "h" | "hr" | "hrs" | "hour" | "hours" => 3_600_000,
        "d" | "day" | "days" => 86_400_000,
        "w" | "wk" | "wks" | "week" | "weeks" => 604_800_000,
        _ => return None,
    };

    count.checked_mul(unit_ms)
}

pub(crate) fn current_epoch_ms() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(i64::MAX)
}
