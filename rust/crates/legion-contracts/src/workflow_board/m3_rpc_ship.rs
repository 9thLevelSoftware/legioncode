//! M3 human-only Ship & Review RPC request and response payloads.

use serde::{Deserialize, Deserializer, Serialize, de};

use crate::ids::WorkflowCardId;

use super::m3_models::{
    WorkflowBoardAcObservationRecord, WorkflowBoardDeliveryStatus,
    WorkflowBoardFindingResolveDisposition, WorkflowBoardManualAcceptanceVerdict,
    WorkflowBoardReviewFinding, WorkflowBoardReviewReport,
};
use super::m3_ship_models::{WorkflowBoardDeliveryReceipt, WorkflowBoardShipReadiness};
use super::m3_validators::{validate_manual_acceptance, validate_resolve_finding};
use super::validators::{
    deserialize_positive_revision, deserialize_present_trimmed_non_empty,
    deserialize_trimmed_non_empty,
};
use super::{WorkflowBoardApprovalReceipt, WorkflowBoardCardStatus, WorkflowCardDetail};

/// Input for `workflowBoard.resolveFinding` (human client only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardResolveFindingInput {
    /// Target workflow card identifier.
    pub card_id: WorkflowCardId,
    /// Expected current card revision (`>= 1`).
    pub expected_revision: u64,
    /// Optional review run ID (defaults to latest active review run).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_id: Option<String>,
    /// Target finding identifier.
    pub finding_id: String,
    /// Human disposition (`fix | accept_risk | dismiss | defer`).
    pub disposition: WorkflowBoardFindingResolveDisposition,
    /// Required non-empty reason when `disposition` is `dismiss` or `accept_risk`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl<'de> Deserialize<'de> for WorkflowBoardResolveFindingInput {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Raw {
            card_id: WorkflowCardId,
            #[serde(deserialize_with = "deserialize_positive_revision")]
            expected_revision: u64,
            #[serde(default, deserialize_with = "deserialize_present_trimmed_non_empty")]
            review_id: Option<String>,
            #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
            finding_id: String,
            disposition: WorkflowBoardFindingResolveDisposition,
            #[serde(default, deserialize_with = "deserialize_present_trimmed_non_empty")]
            reason: Option<String>,
        }
        let raw = Raw::deserialize(deserializer)?;
        validate_resolve_finding(raw.disposition, raw.reason.as_deref())
            .map_err(de::Error::custom)?;
        Ok(Self {
            card_id: raw.card_id,
            expected_revision: raw.expected_revision,
            review_id: raw.review_id,
            finding_id: raw.finding_id,
            disposition: raw.disposition,
            reason: raw.reason,
        })
    }
}

/// Output for `workflowBoard.resolveFinding`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardResolveFindingResult {
    /// Card identifier.
    pub card_id: WorkflowCardId,
    /// Resulting card revision (`>= 1`).
    #[serde(deserialize_with = "deserialize_positive_revision")]
    pub revision: u64,
    /// Updated review finding.
    pub finding: WorkflowBoardReviewFinding,
    /// Updated review report.
    pub review_report: WorkflowBoardReviewReport,
    /// Full updated card detail.
    pub card: WorkflowCardDetail,
}

/// Input for `workflowBoard.recordManualAcceptance` (human client only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardRecordManualAcceptanceInput {
    /// Target workflow card identifier.
    pub card_id: WorkflowCardId,
    /// Expected current card revision (`>= 1`).
    pub expected_revision: u64,
    /// Acceptance criterion identifier (`AC-*`).
    pub criterion_id: String,
    /// Recorded verdict (`pass | fail | not-applicable`).
    pub verdict: WorkflowBoardManualAcceptanceVerdict,
    /// Observation note (required non-empty for `fail` and `not-applicable`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Optional external/manual evidence reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_ref: Option<String>,
    /// Optional attached evidence digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_digest: Option<String>,
    /// Optional expected product snapshot digest precondition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_product_snapshot_digest: Option<String>,
}

impl<'de> Deserialize<'de> for WorkflowBoardRecordManualAcceptanceInput {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Raw {
            card_id: WorkflowCardId,
            #[serde(deserialize_with = "deserialize_positive_revision")]
            expected_revision: u64,
            #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
            criterion_id: String,
            verdict: WorkflowBoardManualAcceptanceVerdict,
            #[serde(default, deserialize_with = "deserialize_present_trimmed_non_empty")]
            note: Option<String>,
            #[serde(default, deserialize_with = "deserialize_present_trimmed_non_empty")]
            evidence_ref: Option<String>,
            #[serde(default, deserialize_with = "deserialize_present_trimmed_non_empty")]
            evidence_digest: Option<String>,
            #[serde(default, deserialize_with = "deserialize_present_trimmed_non_empty")]
            expected_product_snapshot_digest: Option<String>,
        }
        let raw = Raw::deserialize(deserializer)?;
        validate_manual_acceptance(raw.verdict, raw.note.as_deref()).map_err(de::Error::custom)?;
        Ok(Self {
            card_id: raw.card_id,
            expected_revision: raw.expected_revision,
            criterion_id: raw.criterion_id,
            verdict: raw.verdict,
            note: raw.note,
            evidence_ref: raw.evidence_ref,
            evidence_digest: raw.evidence_digest,
            expected_product_snapshot_digest: raw.expected_product_snapshot_digest,
        })
    }
}

/// Output for `workflowBoard.recordManualAcceptance`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardRecordManualAcceptanceResult {
    /// Card identifier.
    pub card_id: WorkflowCardId,
    /// Resulting card revision (`>= 1`).
    #[serde(deserialize_with = "deserialize_positive_revision")]
    pub revision: u64,
    /// Updated AC observation record.
    pub observation: WorkflowBoardAcObservationRecord,
    /// Recomputed Ship readiness.
    pub ship_readiness: WorkflowBoardShipReadiness,
    /// Full updated card detail.
    pub card: WorkflowCardDetail,
}

/// Input for `workflowBoard.approveDelivery` (human client only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardApproveDeliveryInput {
    /// Target workflow card identifier.
    pub card_id: WorkflowCardId,
    /// Expected current card revision (`>= 1`).
    #[serde(deserialize_with = "deserialize_positive_revision")]
    pub expected_revision: u64,
    /// Optional expected product snapshot digest precondition.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_trimmed_non_empty"
    )]
    pub expected_product_snapshot_digest: Option<String>,
    /// Optional delivery note.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_trimmed_non_empty"
    )]
    pub note: Option<String>,
}

/// Output for `workflowBoard.approveDelivery`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardApproveDeliveryResult {
    /// Card identifier.
    pub card_id: WorkflowCardId,
    /// Resulting card revision (`>= 1`).
    #[serde(deserialize_with = "deserialize_positive_revision")]
    pub revision: u64,
    /// Resulting card status (`completed`).
    pub status: WorkflowBoardCardStatus,
    /// Delivery status (`shipped`).
    pub delivery_status: WorkflowBoardDeliveryStatus,
    /// Durable Ship delivery receipt.
    pub delivery_receipt: WorkflowBoardDeliveryReceipt,
    /// Bound delivery approval receipt.
    pub delivery_approval: WorkflowBoardApprovalReceipt,
    /// Full updated card detail.
    pub card: WorkflowCardDetail,
}
