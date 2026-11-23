//! Extended `legioncode thread` wait/error/launch helpers (`#39849`, `#38874`, `#40360`).

use super::types::{
    ThreadDetail, ThreadReadRequest, ThreadRunSummary, ThreadTimelineItem, ThreadWaitRequest,
};
use super::{ThreadClientError, ThreadMcpClient, ThreadWaitOutcome};

/// Terminal run status strings matching `TERMINAL_RUN_STATUSES` in `cli/thread.ts`.
pub const TERMINAL_RUN_STATUSES: &[&str] = &[
    "completed",
    "failed",
    "cancelled",
    "interrupted",
    "rolled_back",
];

impl ThreadMcpClient {
    /// Reads full item text across UTF-16 `textOffset` chunks when truncated (`thread.ts:482-502`).
    pub async fn read_full_item_text(
        &self,
        thread_id: &str,
        item: &ThreadTimelineItem,
    ) -> Result<Option<String>, ThreadClientError> {
        let Some(mut text) = item.text.clone() else {
            return Ok(None);
        };
        let mut truncated = item.text_truncated;
        let mut offset = item.next_text_offset;
        let mut pages = 0usize;
        while truncated {
            let Some(next_offset) = offset else {
                break;
            };
            if pages >= 16 {
                break;
            }
            pages += 1;
            let page = self
                .read_thread(&ThreadReadRequest {
                    thread_id: thread_id.to_owned(),
                    item_id: Some(item.item_id.clone()),
                    text_offset: Some(next_offset),
                    limit: Some(1),
                    ..ThreadReadRequest::default()
                })
                .await?;
            let Some(chunk) = page.items.into_iter().find(|it| it.item_id == item.item_id) else {
                break;
            };
            if let Some(part) = chunk.text {
                text.push_str(&part);
            }
            truncated = chunk.text_truncated;
            offset = chunk.next_text_offset;
        }
        Ok(Some(text))
    }

    /// Reads the last text item of `item_type` (`"assistant_message"` or `"error"`) for `run_id`.
    pub async fn run_result_text(
        &self,
        thread_id: &str,
        run_id: &str,
        item_type: &str,
    ) -> Result<Option<String>, ThreadClientError> {
        let view = if item_type == "error" {
            Some("activity".to_owned())
        } else {
            None
        };
        let page = self
            .read_thread(&ThreadReadRequest {
                thread_id: thread_id.to_owned(),
                view,
                limit: Some(super::THREAD_READ_PAGE_LIMIT),
                run_limit: Some(50),
                ..ThreadReadRequest::default()
            })
            .await?;
        let found = page
            .items
            .iter()
            .rfind(|it| it.item_type == item_type && it.run_id.as_deref() == Some(run_id));
        let Some(item) = found else {
            return Ok(None);
        };
        self.read_full_item_text(thread_id, item).await
    }

    /// Returns the latest run's last error when it ended without completing (`#39849`).
    pub async fn latest_run_error(
        &self,
        thread: &ThreadDetail,
    ) -> Result<Option<String>, ThreadClientError> {
        let Some(latest_run_id) = thread.latest_run_id.as_deref() else {
            return Ok(None);
        };
        if thread.status == "idle"
            || thread.status == "completed"
            || !TERMINAL_RUN_STATUSES.contains(&thread.status.as_str())
        {
            return Ok(None);
        }
        self.run_result_text(&thread.thread_id, latest_run_id, "error")
            .await
    }

    /// Waits for a run and any subsequent woken runs / delegated work, preserving `output` (`#40360`).
    pub async fn wait_for_run_settled(
        &self,
        thread_id: &str,
        run_id: &str,
        slice_ms: u64,
        until_output: bool,
    ) -> Result<ThreadWaitOutcome, ThreadClientError> {
        let initial = self
            .wait_for_run_with_output(thread_id, run_id, slice_ms, until_output)
            .await?;
        let ThreadWaitOutcome::Ended {
            run_id: initial_run_id,
            status: initial_status,
            output: mut carried_output,
        } = initial
        else {
            return Ok(ThreadWaitOutcome::NeedsInput);
        };

        let Some(final_run_id) = initial_run_id.clone() else {
            return Ok(ThreadWaitOutcome::Ended {
                run_id: initial_run_id,
                status: initial_status,
                output: carried_output,
            });
        };

        let mut work_outlived = false;
        let mut settled_outcome = ThreadWaitOutcome::Ended {
            run_id: Some(final_run_id.clone()),
            status: initial_status.clone(),
            output: carried_output.clone(),
        };

        loop {
            let waited = self
                .wait_thread(&ThreadWaitRequest {
                    thread_id: thread_id.to_owned(),
                    run_id: Some(final_run_id.clone()),
                    timeout_ms: Some(slice_ms),
                    until_delegated_work_settled: Some(true),
                    until_output: until_output.then_some(true),
                })
                .await?;
            if let Some(woken_output) = waited.output.clone() {
                carried_output = Some(woken_output.clone());
                if let ThreadWaitOutcome::Ended { ref mut output, .. } = settled_outcome {
                    *output = Some(woken_output);
                }
            }

            let later = self.runs_after(thread_id, &final_run_id).await?;
            if later.has_later_sent {
                return Ok(settled_outcome);
            }
            if !waited.timed_out {
                if let Some(result_run) = waited.result_run.as_ref()
                    && later.later.iter().any(|r| r.run_id == result_run.run_id)
                {
                    return Ok(ThreadWaitOutcome::Ended {
                        run_id: Some(result_run.run_id.clone()),
                        status: result_run.status.clone(),
                        output: carried_output,
                    });
                }
                if !work_outlived {
                    return Ok(settled_outcome);
                }
                if later
                    .later
                    .iter()
                    .any(|r| !TERMINAL_RUN_STATUSES.contains(&r.status.as_str()))
                {
                    continue;
                }
                return Ok(settled_outcome);
            }
            work_outlived = true;
            let snap = self
                .read_thread(&ThreadReadRequest {
                    thread_id: thread_id.to_owned(),
                    limit: Some(1),
                    run_limit: Some(1),
                    ..ThreadReadRequest::default()
                })
                .await?;
            if snap.thread.pending_request_count > 0 {
                return Ok(ThreadWaitOutcome::NeedsInput);
            }
        }
    }

    async fn runs_after(
        &self,
        thread_id: &str,
        after_run_id: &str,
    ) -> Result<RunsAfterSlice, ThreadClientError> {
        let snap = self
            .read_thread(&ThreadReadRequest {
                thread_id: thread_id.to_owned(),
                limit: Some(1),
                run_limit: Some(50),
                ..ThreadReadRequest::default()
            })
            .await?;
        let idx = snap
            .recent_runs
            .iter()
            .position(|r| r.run_id == after_run_id);
        let later: Vec<ThreadRunSummary> = match idx {
            Some(pos) => snap.recent_runs[..pos].to_vec(),
            None => Vec::new(),
        };
        Ok(RunsAfterSlice {
            has_later_sent: false,
            later,
        })
    }
}

struct RunsAfterSlice {
    has_later_sent: bool,
    later: Vec<ThreadRunSummary>,
}
