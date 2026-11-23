//! Local-only read-only feedback report listing (`legioncode feedback list`).

use std::path::Path;

use legion_persistence::paths::ServerDerivedPaths;
use legion_workflows::feedback_types::{FEEDBACK_OFF_MESSAGE, FeedbackReport};
use legion_workflows::trace_types::FeedbackKind;
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

use crate::cmd_thread::ops::print_json_value;
use crate::fallback::{resolve_base_dir, write_stdout_line};
use crate::serve::rpc::server_settings::read_settings_json;

pub(super) fn run_list(base_dir_arg: Option<&Path>, json_out: bool) -> i32 {
    let (base_dir, is_explicit) = resolve_base_dir(base_dir_arg.and_then(Path::to_str));
    let paths = ServerDerivedPaths::derive(&base_dir, false, is_explicit);
    let mode = read_settings_json(&paths.settings_path)
        .ok()
        .and_then(|s| {
            s.pointer("/agentFeedback/mode")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "off".to_owned());
    if mode == "off" {
        eprintln!("{FEEDBACK_OFF_MESSAGE}");
        return 1;
    }
    let reports = match read_reports_readonly(&paths.db_path) {
        Ok(list) => list,
        Err(msg) => {
            eprintln!("Could not read this machine's feedback reports: {msg}");
            return 1;
        }
    };
    if json_out {
        return print_json_value(&reports);
    }
    for r in &reports {
        let summary = format!(
            "{}  {}  {}  {}  {}  {}",
            r.created_at,
            r.thread_id.as_deref().unwrap_or("-"),
            r.kind.as_str(),
            if r.subject.is_empty() {
                "-"
            } else {
                &r.subject
            },
            if r.user_requested {
                "approved"
            } else {
                "unprompted"
            },
            if r.sent { "sent" } else { "local" }
        );
        if write_stdout_line(&summary, "feedback list") != 0
            || write_stdout_line(&format!("  expected: {}", r.expected), "feedback list") != 0
            || write_stdout_line(&format!("  actual: {}", r.actual), "feedback list") != 0
        {
            return 1;
        }
    }
    0
}

pub(crate) fn read_reports_readonly(db_path: &Path) -> Result<Vec<FeedbackReport>, String> {
    if !db_path.is_file() {
        return Ok(Vec::new());
    }
    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| e.to_string())?;
    let exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table' AND name = 'agent_feedback'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if exists == 0 {
        return Ok(Vec::new());
    }
    let mut stmt = conn
        .prepare(
            "SELECT id, thread_id, created_at, kind, subject, expected, actual, workaround,
                    user_requested, trace, source, sent
             FROM agent_feedback
             ORDER BY created_at DESC, id DESC
             LIMIT 50",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            let kind_str: String = row.get(3)?;
            let kind =
                serde_json::from_value(Value::String(kind_str)).unwrap_or(FeedbackKind::Other);
            let trace_raw: Option<String> = row.get(9)?;
            let trace = trace_raw.and_then(|s| serde_json::from_str(&s).ok());
            let src_raw: String = row.get(10)?;
            let source = legion_workflows::feedback_types::FeedbackSource::parse(&src_raw)
                .unwrap_or(legion_workflows::feedback_types::FeedbackSource::Cli);
            Ok(FeedbackReport {
                id: row.get(0)?,
                thread_id: row.get(1)?,
                created_at: row.get(2)?,
                kind,
                subject: row.get(4)?,
                expected: row.get(5)?,
                actual: row.get(6)?,
                workaround: row.get(7)?,
                user_requested: row.get::<_, i64>(8)? != 0,
                trace,
                source,
                sent: row.get::<_, i64>(11)? != 0,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}
