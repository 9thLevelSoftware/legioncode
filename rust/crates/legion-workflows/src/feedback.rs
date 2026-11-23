#![forbid(unsafe_code)]
//! Strictly local-only agent feedback persistence and approval service (`ADR-013`, `#35871`).
//! Never performs HTTP requests, telemetry uploads, or remote forwarding (`sent = 0` always).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, OptionalExtension, params};
use uuid::Uuid;

use crate::feedback_types::{
    AgentFeedbackAnswerRequest, FEEDBACK_AWAITING_APPROVAL_NOTE, FEEDBACK_LIST_LIMIT,
    FeedbackAnswerChoice, FeedbackError, FeedbackMode, FeedbackPreview, FeedbackReport,
    FeedbackRequestStatus, FeedbackSource, FeedbackSubmitOutcome,
    MAX_REPORTS_OUTSIDE_TURNS_PER_HOUR, MAX_REPORTS_PER_TURN, PendingFeedbackCard,
    SubmitFeedbackInput, check_allowed, decode_feedback_row, resolve_subject, row_from_sqlite,
};
use crate::schema::ensure_workflow_and_feedback_schema;
use crate::scrub::scrub_feedback;
use crate::time::{format_millis_iso, now_epoch_ms, now_iso_utc};
use crate::trace::build_feedback_trace;
use crate::trace_types::BuildFeedbackTraceInput;

/// SQLite-backed local-only `agent_feedback` store and approval card tracker.
#[derive(Clone)]
pub struct FeedbackStore {
    conn: Arc<Mutex<Connection>>,
    cards: Arc<Mutex<HashMap<String, PendingFeedbackCard>>>,
}

impl FeedbackStore {
    /// Opens the local-only `agent_feedback` store and ensures schema readiness.
    pub fn new(conn: Arc<Mutex<Connection>>) -> Result<Self, FeedbackError> {
        {
            let guard = conn
                .lock()
                .map_err(|_| FeedbackError::Storage("connection lock poisoned".to_owned()))?;
            ensure_workflow_and_feedback_schema(&guard)
                .map_err(|e| FeedbackError::Storage(e.to_string()))?;
        }
        Ok(Self {
            conn,
            cards: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Validates, PII-scrubs, and either persists locally (`sent = 0`) or stages an approval card.
    pub fn submit(
        &self,
        mode: FeedbackMode,
        input: &SubmitFeedbackInput<'_>,
    ) -> Result<FeedbackSubmitOutcome, FeedbackError> {
        check_allowed(mode, input.user_requested, input.source)?;
        let subject = resolve_subject(input.subject, input.registered_tools, input.cli_commands)
            .ok_or(FeedbackError::InvalidSubject)?;

        if input.user_requested && (input.thread_id.is_none() || input.run_id.is_none()) {
            return Err(FeedbackError::ApprovalContextMissing);
        }

        let expected = scrub_feedback(input.expected, input.sensitive);
        let actual = scrub_feedback(input.actual, input.sensitive);
        let workaround = scrub_feedback(input.workaround, input.sensitive);
        let id = Uuid::new_v4().to_string();

        if !input.user_requested {
            let report = FeedbackReport {
                id,
                thread_id: input.thread_id.map(str::to_owned),
                created_at: now_iso_utc(),
                kind: input.kind,
                subject,
                expected,
                actual,
                workaround,
                user_requested: false,
                trace: None,
                source: input.source,
                sent: false,
            };
            self.insert_local_report(mode, &report, input.run_id)?;
            return Ok(FeedbackSubmitOutcome::Stored(report));
        }

        let thread_id = input
            .thread_id
            .ok_or(FeedbackError::ApprovalContextMissing)?;
        let run_id = input.run_id.ok_or(FeedbackError::ApprovalContextMissing)?;
        let outline = build_feedback_trace(&BuildFeedbackTraceInput {
            runs: input.trace_runs,
            items: input.trace_items,
            turns: input.turns.max(1),
            with_text: false,
            sensitive: input.sensitive,
            registered_tools: input.registered_tools,
            submitting_run_id: run_id,
        });
        let conversation = build_feedback_trace(&BuildFeedbackTraceInput {
            runs: input.trace_runs,
            items: input.trace_items,
            turns: input.turns.max(1),
            with_text: true,
            sensitive: input.sensitive,
            registered_tools: input.registered_tools,
            submitting_run_id: run_id,
        });
        let conv_opt = (conversation.text_turns > 0).then_some(conversation);
        let preview = FeedbackPreview {
            kind: input.kind,
            subject,
            expected,
            actual,
            workaround,
            trace: Some(outline),
        };
        let turn_item_id = format!("turn-item:feedback-request:{id}");
        self.show_card(PendingFeedbackCard {
            report_id: id.clone(),
            thread_id: thread_id.to_owned(),
            run_id: run_id.to_owned(),
            turn_item_id: turn_item_id.clone(),
            preview: preview.clone(),
            conversation_trace: conv_opt.clone(),
            status: FeedbackRequestStatus::Pending,
        })?;

        Ok(FeedbackSubmitOutcome::AwaitingApproval {
            report_id: id,
            turn_item_id,
            preview,
            conversation_trace: conv_opt,
            note: FEEDBACK_AWAITING_APPROVAL_NOTE,
        })
    }

    /// Stages a requested feedback approval card enforcing the `3`-cards-per-turn cap.
    pub fn show_card(&self, card: PendingFeedbackCard) -> Result<(), FeedbackError> {
        let mut cards = self
            .cards
            .lock()
            .map_err(|_| FeedbackError::Storage("cards lock poisoned".to_owned()))?;
        let turn_count = cards
            .values()
            .filter(|c| c.thread_id == card.thread_id && c.run_id == card.run_id)
            .count();
        let count_i64 = i64::try_from(turn_count).unwrap_or(i64::MAX);
        if count_i64 >= MAX_REPORTS_PER_TURN {
            return Err(FeedbackError::LimitReached { scope: "cards" });
        }
        cards.insert(card.turn_item_id.clone(), card);
        Ok(())
    }

    /// Resolves `agentFeedback.answerRequest`, persisting approved reports locally with `sent = 0`.
    pub fn answer_request(
        &self,
        mode: FeedbackMode,
        req: &AgentFeedbackAnswerRequest,
    ) -> Result<Option<FeedbackReport>, FeedbackError> {
        let card_snapshot = {
            let cards = self
                .cards
                .lock()
                .map_err(|_| FeedbackError::Storage("cards lock poisoned".to_owned()))?;
            let card = cards
                .get(&req.turn_item_id)
                .ok_or(FeedbackError::NotFound)?;
            if card.thread_id != req.thread_id {
                return Err(FeedbackError::NotFound);
            }
            if card.status != FeedbackRequestStatus::Pending {
                return Err(FeedbackError::AlreadyAnswered);
            }
            card.clone()
        };

        if req.answer == FeedbackAnswerChoice::Decline {
            let mut cards = self
                .cards
                .lock()
                .map_err(|_| FeedbackError::Storage("cards lock poisoned".to_owned()))?;
            if let Some(c) = cards.get_mut(&req.turn_item_id) {
                c.status = FeedbackRequestStatus::Declined;
            }
            return Ok(None);
        }

        if mode == FeedbackMode::Off {
            let mut cards = self
                .cards
                .lock()
                .map_err(|_| FeedbackError::Storage("cards lock poisoned".to_owned()))?;
            if let Some(c) = cards.get_mut(&req.turn_item_id) {
                c.status = FeedbackRequestStatus::Cancelled;
            }
            return Err(FeedbackError::RequestFeedbackOff);
        }

        let trace = if req.include_conversation {
            card_snapshot
                .conversation_trace
                .clone()
                .or(card_snapshot.preview.trace.clone())
        } else {
            card_snapshot.preview.trace.clone()
        };

        let report = FeedbackReport {
            id: card_snapshot.report_id.clone(),
            thread_id: Some(req.thread_id.clone()),
            created_at: now_iso_utc(),
            kind: card_snapshot.preview.kind,
            subject: card_snapshot.preview.subject.clone(),
            expected: card_snapshot.preview.expected.clone(),
            actual: card_snapshot.preview.actual.clone(),
            workaround: card_snapshot.preview.workaround.clone(),
            user_requested: true,
            trace,
            source: FeedbackSource::Mcp,
            sent: false,
        };
        self.insert_local_report(mode, &report, Some(&card_snapshot.run_id))?;

        let mut cards = self
            .cards
            .lock()
            .map_err(|_| FeedbackError::Storage("cards lock poisoned".to_owned()))?;
        if let Some(c) = cards.get_mut(&req.turn_item_id) {
            c.preview.trace.clone_from(&report.trace);
            c.status = FeedbackRequestStatus::Sent;
        }
        Ok(Some(report))
    }

    /// Reads the `50` most recent reports directly from SQLite, newest first.
    pub fn read_reports(&self) -> Result<Vec<FeedbackReport>, FeedbackError> {
        let guard = self
            .conn
            .lock()
            .map_err(|_| FeedbackError::Storage("connection lock poisoned".to_owned()))?;
        let exists: i64 = guard
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table' AND name = 'agent_feedback'",
                [],
                |row| row.get(0),
            )
            .map_err(|e| FeedbackError::Storage(e.to_string()))?;
        if exists == 0 {
            return Ok(Vec::new());
        }
        let mut stmt = guard
            .prepare(
                "SELECT id, thread_id, created_at, kind, subject, expected, actual, workaround,
                        user_requested, trace, source, sent
                 FROM agent_feedback
                 ORDER BY rowid DESC
                 LIMIT ?1",
            )
            .map_err(|e| FeedbackError::Storage(e.to_string()))?;
        let limit = i64::try_from(FEEDBACK_LIST_LIMIT).unwrap_or(50);
        let rows = stmt
            .query_map(params![limit], row_from_sqlite)
            .map_err(|e| FeedbackError::Storage(e.to_string()))?;

        let mut out = Vec::new();
        for row_res in rows {
            let raw = row_res.map_err(|e| FeedbackError::Storage(e.to_string()))?;
            out.push(decode_feedback_row(raw)?);
        }
        Ok(out)
    }

    /// Lists recent feedback reports or rejects with [`FeedbackError::Off`] when mode is `off`.
    pub fn list_reports(&self, mode: FeedbackMode) -> Result<Vec<FeedbackReport>, FeedbackError> {
        if mode == FeedbackMode::Off {
            return Err(FeedbackError::Off);
        }
        self.read_reports()
    }

    fn insert_local_report(
        &self,
        mode: FeedbackMode,
        report: &FeedbackReport,
        run_id: Option<&str>,
    ) -> Result<(), FeedbackError> {
        check_allowed(mode, report.user_requested, report.source)?;
        let trace_json = report
            .trace
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| FeedbackError::Storage(e.to_string()))?;
        let mut guard = self
            .conn
            .lock()
            .map_err(|_| FeedbackError::Storage("connection lock poisoned".to_owned()))?;
        let tx = guard
            .transaction()
            .map_err(|e| FeedbackError::Storage(e.to_string()))?;

        if !report.user_requested {
            if let Some(rid) = run_id {
                let count: i64 = tx
                    .query_row(
                        "SELECT COUNT(*) FROM agent_feedback WHERE run_id = ?1 AND user_requested = 0",
                        params![rid],
                        |r| r.get(0),
                    )
                    .map_err(|e| FeedbackError::Storage(e.to_string()))?;
                if count >= MAX_REPORTS_PER_TURN {
                    return Err(FeedbackError::LimitReached { scope: "turn" });
                }
            } else {
                let since = format_millis_iso(now_epoch_ms().saturating_sub(3_600_000));
                let count: i64 = tx
                    .query_row(
                        "SELECT COUNT(*) FROM agent_feedback WHERE run_id IS NULL AND created_at > ?1",
                        params![since],
                        |r| r.get(0),
                    )
                    .map_err(|e| FeedbackError::Storage(e.to_string()))?;
                if count >= MAX_REPORTS_OUTSIDE_TURNS_PER_HOUR {
                    return Err(FeedbackError::LimitReached { scope: "hour" });
                }
            }
        }

        let existing: Option<String> = tx
            .query_row(
                "SELECT id FROM agent_feedback WHERE id = ?1",
                params![&report.id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| FeedbackError::Storage(e.to_string()))?;
        if existing.is_some() {
            return Err(FeedbackError::AlreadyAnswered);
        }

        // ADR-013: `sent` MUST remain `0` ALWAYS (strictly local-only storage).
        tx.execute(
            "INSERT INTO agent_feedback
             (id, thread_id, run_id, created_at, kind, subject, expected, actual, workaround, user_requested, trace, source, sent)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 0)",
            params![
                report.id,
                report.thread_id,
                run_id,
                report.created_at,
                report.kind.as_str(),
                report.subject,
                report.expected,
                report.actual,
                report.workaround,
                i64::from(report.user_requested),
                trace_json,
                report.source.as_str(),
            ],
        )
        .map_err(|e| FeedbackError::Storage(e.to_string()))?;
        tx.commit()
            .map_err(|e| FeedbackError::Storage(e.to_string()))
    }
}
