#![forbid(unsafe_code)]
//! Review payload parsing and shared severity normalization for background turn settlement.

use legion_contracts::workflow_board::normalize_review_finding_severity;
use legion_workflows::traits::{AgentEnding, AgentResult};
use serde::Deserialize;
use uuid::Uuid;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ParsedReviewPayload {
    pub verdict: String,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub findings: Vec<ParsedReviewFinding>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ParsedReviewFinding {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub severity: String,
    #[serde(default)]
    pub criterion_id: Option<String>,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub line: Option<i64>,
    pub message: String,
}

pub(crate) fn parse_review_settlement(res: &AgentResult) -> ParsedReviewPayload {
    if res.ending != AgentEnding::Completed {
        return ParsedReviewPayload {
            verdict: "failed".to_owned(),
            summary: Some(format!("Review turn ended with `{}`", res.detail)),
            findings: Vec::new(),
        };
    }
    if let Some(ref val) = res.output
        && let Ok(parsed) = serde_json::from_value::<ParsedReviewPayload>(val.clone())
        && matches!(parsed.verdict.as_str(), "pass" | "findings" | "failed")
    {
        return normalize_review_payload(parsed);
    }
    if let Ok(parsed) = serde_json::from_str::<ParsedReviewPayload>(res.reply.trim())
        && matches!(parsed.verdict.as_str(), "pass" | "findings" | "failed")
    {
        return normalize_review_payload(parsed);
    }
    ParsedReviewPayload {
        verdict: "failed".to_owned(),
        summary: Some(
            "Independent review failed: unstructured reply without structured review JSON or `legion_workflow_board_propose`"
                .to_owned(),
        ),
        findings: Vec::new(),
    }
}

fn normalize_review_payload(mut parsed: ParsedReviewPayload) -> ParsedReviewPayload {
    for f in &mut parsed.findings {
        if f.id.trim().is_empty() {
            f.id = format!("fnd-{}", Uuid::new_v4().simple());
        } else {
            f.id = f.id.trim().to_owned();
        }
        f.severity = normalize_review_finding_severity(Some(&f.severity)).to_owned();
    }
    if parsed.verdict == "pass" && !parsed.findings.is_empty() {
        "findings".clone_into(&mut parsed.verdict);
    }
    parsed
}
