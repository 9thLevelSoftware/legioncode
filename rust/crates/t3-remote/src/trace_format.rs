//! CLI formatting helpers for OTLP trace summaries matching `apps/server/src/cli/trace.ts`.

use std::path::Path;

use crate::trace::TraceSummary;

const HEADER: [&str; 8] = [
    "span",
    "count",
    "/min",
    "p50",
    "p90",
    "max",
    "interrupted",
    "failed",
];

/// Formats milliseconds matching `cli/trace.ts`: `<1000ms` as rounded `Nms`, otherwise tenths of seconds (`1s`, `1.3s`).
///
/// ```
/// use t3_remote::trace::format_duration_ms;
///
/// assert_eq!(format_duration_ms(999.4), "999ms");
/// assert_eq!(format_duration_ms(999.6), "1s");
/// assert_eq!(format_duration_ms(1250.0), "1.3s");
/// ```
#[must_use]
pub fn format_duration_ms(ms: f64) -> String {
    if !ms.is_finite() {
        return String::from("0ms");
    }
    let rounded = (ms + 0.5).floor();
    if rounded < 1000.0 {
        let formatted = format!("{rounded:.0}");
        let clean = if formatted == "-0" { "0" } else { &formatted };
        return format!("{clean}ms");
    }

    let tenths = ((ms / 100.0) + 0.5).floor();
    let tenths_str = format!("{tenths:.0}");
    if tenths_str.len() <= 1 {
        return format!("0.{tenths_str}s");
    }
    let split = tenths_str.len() - 1;
    let (whole, frac) = tenths_str.split_at(split);
    if frac == "0" {
        format!("{whole}s")
    } else {
        format!("{whole}.{frac}s")
    }
}

/// Formats a non-empty [`TraceSummary`] table matching `apps/server/src/cli/trace.ts` byte-for-byte.
///
/// Returns no trailing newline, matching `formatTraceSummary` in TypeScript.
#[must_use]
pub fn format_trace_summary(summary: &TraceSummary, limit: usize) -> String {
    let first_iso = summary
        .first_end_ms
        .map_or_else(|| String::from("-"), format_epoch_ms_iso);
    let last_iso = summary
        .last_end_ms
        .map_or_else(|| String::from("-"), format_epoch_ms_iso);
    let minutes_fixed = format!("{:.1}", summary.minutes);
    let minutes_str = minutes_fixed.strip_suffix(".0").unwrap_or(&minutes_fixed);

    let mut lines = vec![format!(
        "{} spans ended from {first_iso} to {last_iso} ({minutes_str} min).",
        summary.span_count
    )];
    if summary.skipped_line_count > 0 {
        lines.push(format!(
            "Skipped {} lines that are not spans.",
            summary.skipped_line_count
        ));
    }
    lines.push(String::new());

    let shown_count = summary.spans.len().min(limit);
    let mut rows: Vec<[String; 8]> = Vec::with_capacity(shown_count + 1);
    rows.push(HEADER.map(str::to_owned));

    for span in summary.spans.iter().take(limit) {
        let rate = match span.per_minute {
            None => String::from("-"),
            Some(r) if r < 0.1 => String::from("<0.1"),
            Some(r) => format!("{r:.1}"),
        };
        rows.push([
            span.name.clone(),
            span.count.to_string(),
            rate,
            format_duration_ms(span.p50_ms),
            format_duration_ms(span.p90_ms),
            format_duration_ms(span.max_ms),
            span.interrupted_count.to_string(),
            span.failure_count.to_string(),
        ]);
    }

    let mut widths = [0usize; 8];
    for row in &rows {
        for (idx, cell) in row.iter().enumerate() {
            widths[idx] = widths[idx].max(cell.chars().count());
        }
    }

    for row in &rows {
        let mut rendered_cols = Vec::with_capacity(8);
        for (idx, cell) in row.iter().enumerate() {
            let char_len = cell.chars().count();
            let pad = widths[idx].saturating_sub(char_len);
            if idx == 0 {
                rendered_cols.push(format!("{cell}{}", " ".repeat(pad)));
            } else {
                rendered_cols.push(format!("{}{cell}", " ".repeat(pad)));
            }
        }
        lines.push(rendered_cols.join("  "));
    }

    if summary.spans.len() > limit {
        lines.push(String::new());
        lines.push(format!(
            "{} more span names. Use --limit to show more.",
            summary.spans.len() - limit
        ));
    }

    lines.join("\n")
}

/// Formats the empty-trace diagnostic message matching `apps/server/src/cli/trace.ts:184-185`.
///
/// ```
/// use std::path::Path;
/// use t3_remote::trace::format_empty_trace_message;
///
/// assert_eq!(
///     format_empty_trace_message(Path::new("/var/log/trace.ndjson"), true, 2),
///     "No spans found in /var/log/trace.ndjson or its rotated files in that window. Skipped 2 lines that are not spans."
/// );
/// ```
#[must_use]
pub fn format_empty_trace_message(
    trace_file_path: &Path,
    has_since_window: bool,
    skipped_line_count: usize,
) -> String {
    let window_suffix = if has_since_window {
        " in that window"
    } else {
        ""
    };
    let skipped_suffix = if skipped_line_count > 0 {
        format!(" Skipped {skipped_line_count} lines that are not spans.")
    } else {
        String::new()
    };
    format!(
        "No spans found in {} or its rotated files{window_suffix}.{skipped_suffix}",
        trace_file_path.display()
    )
}

/// Formats UTC epoch milliseconds as ISO-8601 `YYYY-MM-DDTHH:MM:SS.fffZ` (matching JS `Date.toISOString()`).
pub(crate) fn format_epoch_ms_iso(epoch_ms: i64) -> String {
    let total_seconds = i128::from(epoch_ms).div_euclid(1_000);
    let millis = i128::from(epoch_ms).rem_euclid(1_000);
    let days = total_seconds.div_euclid(86_400);
    let secs_of_day = total_seconds.rem_euclid(86_400);

    let hour = secs_of_day / 3_600;
    let minute = (secs_of_day % 3_600) / 60;
    let second = secs_of_day % 60;

    // Howard Hinnant's civil_from_days algorithm.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let year = y + i128::from(m <= 2);

    format!("{year:04}-{m:02}-{d:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}
