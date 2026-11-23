#![forbid(unsafe_code)]
//! Strict local-only (`ADR-013`, `#35871`) feedback scrubber, trace builder, and store tests.

use std::sync::{Arc, Mutex};

use legion_workflows::feedback::FeedbackStore;
use legion_workflows::feedback_types::{
    AgentFeedbackAnswerRequest, FEEDBACK_AWAITING_APPROVAL_NOTE, FeedbackAnswerChoice,
    FeedbackError, FeedbackMode, FeedbackSubmitOutcome, MAX_REPORTS_OUTSIDE_TURNS_PER_HOUR,
    MAX_REPORTS_PER_TURN, SubmitFeedbackInput,
};
use legion_workflows::schema::ensure_workflow_and_feedback_schema;
use legion_workflows::scrub::{
    FEEDBACK_TEXT_MAX_CHARS, REDACTED, scrub_feedback, scrub_feedback_text,
};
use legion_workflows::trace::build_feedback_trace;
use legion_workflows::trace_types::{
    BuildFeedbackTraceInput, FEEDBACK_TRACE_MAX_BYTES, FeedbackKind, FeedbackTraceEntryKind,
    TraceItemInput, TraceItemKind, TraceItemStatus, TraceRunInput, resolve_feedback_tool_name,
};
use rusqlite::Connection;

type TestResult = Result<(), Box<dyn std::error::Error>>;
type DbPair = (Arc<Mutex<Connection>>, FeedbackStore);

fn open_feedback_db() -> Result<DbPair, Box<dyn std::error::Error>> {
    let conn = Arc::new(Mutex::new(Connection::open_in_memory()?));
    {
        let g = conn.lock().map_err(|_| "lock poisoned")?;
        ensure_workflow_and_feedback_schema(&g)?;
    }
    let store = FeedbackStore::new(Arc::clone(&conn))?;
    Ok((conn, store))
}

#[test]
fn scrub_redacts_secrets_paths_credentials_and_caps_300_codepoints() {
    let scrubbed = scrub_feedback(
        "See https://docs.example.com/guide and https://alice:secret@git.example.com/org/repo for user alice@example.com with sk-1234567890abcdef and ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ12",
        &["alice", "worktree-feature-1"],
    );
    assert!(!scrubbed.contains("secret"));
    assert!(!scrubbed.contains("alice@example.com"));
    assert!(!scrubbed.contains("sk-1234567890abcdef"));
    assert!(!scrubbed.contains("ghp_"));
    assert!(scrubbed.contains("docs.example.com/guide"));

    let paths = scrub_feedback(
        r#"Edited "/Users/bob/proj/src/lib.rs" and C:\Users\bob\repo\main.rs plus /home/bob/work"#,
        &[],
    );
    assert!(!paths.contains("/Users/bob"));
    assert!(!paths.contains(r"C:\Users\bob"));
    assert!(!paths.contains("/home/bob"));

    // Known multiline value & kebab-case identifier preservation.
    let known = scrub_feedback(
        "Branch legion-code-main on\n/repo/secret-path\ncrash",
        &["/repo/secret-path"],
    );
    assert!(!known.contains("secret-path"));
    assert!(known.contains("legion-code-main"));

    // High-entropy token redaction.
    let entropy = scrub_feedback("id: a8F3kL9mP2xQ7zR4wV6nB1cD5eG0hJ9K", &[]);
    assert_eq!(entropy, format!("id: {REDACTED}"));

    // Surrogate/emoji-safe 300-codepoint limit + secret straddling scan cut line never leaks.
    let emojis: String = std::iter::repeat_n("🚀x", 200).collect();
    let capped = scrub_feedback_text(&emojis, &[], FEEDBACK_TEXT_MAX_CHARS);
    assert!(capped.cut);
    assert_eq!(capped.text.chars().count(), FEEDBACK_TEXT_MAX_CHARS);

    let prefix = "ok ".repeat(1_000);
    let straddling = format!("{prefix}ghp_1234567890abcdef1234567890abcdef");
    let cut_res = scrub_feedback_text(&straddling, &[], FEEDBACK_TEXT_MAX_CHARS);
    assert!(cut_res.cut);
    assert!(!cut_res.text.contains("ghp_"));
}

fn sample_trace_inputs() -> (Vec<TraceRunInput>, Vec<TraceItemInput>) {
    let runs = vec![TraceRunInput {
        id: "turn-1".to_owned(),
        ordinal: 1,
        started_at: Some("2026-01-01T00:00:00.000Z".to_owned()),
        driver: Some("codex".to_owned()),
    }];
    let big_user_msg = "hello ".repeat(6_000);
    let mut items = vec![
        TraceItemInput {
            run_id: Some("turn-1".to_owned()),
            parent_item_id: None,
            ordinal: 1,
            kind: TraceItemKind::UserMessage,
            status: TraceItemStatus::Completed,
            text: Some(big_user_msg),
            tool_name: None,
            tool_is_integration: false,
            exit_code: None,
            output_indicates_failure: false,
            started_at: None,
            completed_at: None,
        },
        TraceItemInput {
            run_id: Some("turn-1".to_owned()),
            parent_item_id: None,
            ordinal: 2,
            kind: TraceItemKind::DynamicTool,
            status: TraceItemStatus::Completed,
            text: None,
            tool_name: Some("legion_feedback".to_owned()),
            tool_is_integration: false,
            exit_code: None,
            output_indicates_failure: false,
            started_at: None,
            completed_at: None,
        },
    ];
    for idx in 0..90 {
        items.push(TraceItemInput {
            run_id: Some("turn-1".to_owned()),
            parent_item_id: None,
            ordinal: 3 + idx,
            kind: TraceItemKind::CommandExecution,
            status: TraceItemStatus::Completed,
            text: None,
            tool_name: None,
            tool_is_integration: false,
            exit_code: Some(0),
            output_indicates_failure: false,
            started_at: Some("2026-01-01T00:00:01.000Z".to_owned()),
            completed_at: Some("2026-01-01T00:00:01.250Z".to_owned()),
        });
    }
    (runs, items)
}

#[test]
fn trace_normalizes_legion_and_t3_tools_and_omits_outline_text() -> TestResult {
    assert_eq!(
        resolve_feedback_tool_name(
            "mcp__legion_code_direct__legion_feedback",
            &["legion_feedback"]
        ),
        Some("legion_feedback".to_owned())
    );
    assert_eq!(
        resolve_feedback_tool_name("mcp__t3_code__t3_feedback", &["t3_feedback"]),
        Some("t3_feedback".to_owned())
    );

    let (runs, items) = sample_trace_inputs();
    let outline = build_feedback_trace(&BuildFeedbackTraceInput {
        runs: &runs,
        items: &items,
        turns: 5,
        with_text: false,
        sensitive: &[],
        registered_tools: &["legion_feedback", "search_files"],
        submitting_run_id: "turn-1",
    });
    assert!(outline.truncated);
    assert_eq!(outline.text_turns, 0);
    assert!(outline.entries.len() <= 80);
    assert!(
        !outline
            .entries
            .iter()
            .any(|e| e.tool.as_deref() == Some("legion_feedback"))
    );
    let user_entry = outline
        .entries
        .iter()
        .find(|e| e.kind == FeedbackTraceEntryKind::User)
        .ok_or("missing user entry")?;
    assert!(user_entry.text.is_none());
    assert!(user_entry.length.is_some());
    Ok(())
}

#[test]
fn trace_bounds_conversation_under_16kb() -> TestResult {
    let (runs, items) = sample_trace_inputs();
    let conv = build_feedback_trace(&BuildFeedbackTraceInput {
        runs: &runs,
        items: &items,
        turns: 5,
        with_text: true,
        sensitive: &[],
        registered_tools: &["legion_feedback"],
        submitting_run_id: "turn-1",
    });
    assert_eq!(conv.text_turns, 1);
    assert!(serde_json::to_vec(&conv)?.len() <= FEEDBACK_TRACE_MAX_BYTES);
    Ok(())
}

fn sample_submit_input(
    req: bool,
    thread_id: Option<&'static str>,
    run_id: Option<&'static str>,
) -> SubmitFeedbackInput<'static> {
    const TOOLS: [&str; 2] = ["legion_feedback", "t3_feedback"];
    const CLI: [&str; 1] = ["legion"];
    const SENSITIVE: [&str; 1] = ["dev"];
    SubmitFeedbackInput {
        kind: FeedbackKind::Bug,
        subject: "mcp__t3_code__t3_feedback",
        expected: "clear output",
        actual: "found token ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ12 at /Users/dev/x.rs",
        workaround: "none",
        user_requested: req,
        turns: 5,
        source: legion_workflows::feedback_types::FeedbackSource::Mcp,
        thread_id,
        run_id,
        sensitive: &SENSITIVE,
        registered_tools: &TOOLS,
        cli_commands: &CLI,
        trace_runs: &[],
        trace_items: &[],
    }
}

#[test]
fn feedback_store_gates_modes_and_approval_always_sets_sent_zero() -> TestResult {
    let (conn, store) = open_feedback_db()?;

    assert!(matches!(
        store.submit(
            FeedbackMode::Off,
            &sample_submit_input(true, Some("t1"), Some("r1"))
        ),
        Err(FeedbackError::Off)
    ));
    assert!(matches!(
        store.submit(
            FeedbackMode::OnRequest,
            &sample_submit_input(false, Some("t1"), Some("r1"))
        ),
        Err(FeedbackError::NotRequested { .. })
    ));
    assert!(matches!(
        store.submit(
            FeedbackMode::OnRequest,
            &sample_submit_input(true, None, None)
        ),
        Err(FeedbackError::ApprovalContextMissing)
    ));

    let FeedbackSubmitOutcome::AwaitingApproval {
        turn_item_id: decl_item,
        preview,
        note,
        ..
    } = store.submit(
        FeedbackMode::OnRequest,
        &sample_submit_input(true, Some("t1"), Some("r1")),
    )?
    else {
        return Err("expected AwaitingApproval".into());
    };
    assert_eq!(note, FEEDBACK_AWAITING_APPROVAL_NOTE);
    assert!(!preview.actual.contains("ghp_"));
    assert!(!preview.actual.contains("/Users/"));

    let declined = store.answer_request(
        FeedbackMode::OnRequest,
        &AgentFeedbackAnswerRequest {
            thread_id: "t1".to_owned(),
            turn_item_id: decl_item,
            answer: FeedbackAnswerChoice::Decline,
            include_conversation: false,
        },
    )?;
    assert!(declined.is_none());
    assert_eq!(store.read_reports()?.len(), 0);

    let FeedbackSubmitOutcome::AwaitingApproval {
        turn_item_id: send_item,
        ..
    } = store.submit(
        FeedbackMode::OnRequest,
        &sample_submit_input(true, Some("t1"), Some("r1")),
    )?
    else {
        return Err("expected second AwaitingApproval".into());
    };
    let saved = store
        .answer_request(
            FeedbackMode::OnRequest,
            &AgentFeedbackAnswerRequest {
                thread_id: "t1".to_owned(),
                turn_item_id: send_item,
                answer: FeedbackAnswerChoice::Send,
                include_conversation: true,
            },
        )?
        .ok_or("expected saved report")?;
    assert!(!saved.sent);
    let reports = store.read_reports()?;
    assert_eq!(reports.len(), 1);
    assert!(!reports[0].sent);
    assert_eq!(reports[0].subject, "t3_feedback");

    let sent_nonzero: i64 = conn.lock().map_err(|_| "lock")?.query_row(
        "SELECT COUNT(*) FROM agent_feedback WHERE sent != 0",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(sent_nonzero, 0);
    Ok(())
}

#[test]
fn feedback_store_enforces_turn_and_hourly_rate_limits() -> TestResult {
    let (_conn, store) = open_feedback_db()?;
    for _ in 0..MAX_REPORTS_PER_TURN {
        assert!(matches!(
            store.submit(
                FeedbackMode::Auto,
                &sample_submit_input(false, Some("t2"), Some("r-limit"))
            ),
            Ok(FeedbackSubmitOutcome::Stored(_))
        ));
    }
    assert!(matches!(
        store.submit(
            FeedbackMode::Auto,
            &sample_submit_input(false, Some("t2"), Some("r-limit"))
        ),
        Err(FeedbackError::LimitReached { scope: "turn" })
    ));

    for _ in 0..MAX_REPORTS_OUTSIDE_TURNS_PER_HOUR {
        assert!(matches!(
            store.submit(FeedbackMode::Auto, &sample_submit_input(false, None, None)),
            Ok(FeedbackSubmitOutcome::Stored(_))
        ));
    }
    assert!(matches!(
        store.submit(FeedbackMode::Auto, &sample_submit_input(false, None, None)),
        Err(FeedbackError::LimitReached { scope: "hour" })
    ));
    Ok(())
}
