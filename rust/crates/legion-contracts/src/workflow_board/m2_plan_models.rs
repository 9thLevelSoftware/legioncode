//! M2 Plan design lenses, alternatives, strategies, and metadata models.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::Value;

use super::m2_validators::{
    deserialize_opt_artifact_key_vec, validate_alternatives, validate_lenses_partial,
};
use super::validators::{
    deserialize_opt_trimmed_non_empty, deserialize_opt_trimmed_non_empty_vec,
    deserialize_trimmed_non_empty,
};

/// One of the six required planning design lenses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowBoardPlanLens {
    /// User experience lens.
    Ux,
    /// Functional requirements lens.
    Functional,
    /// System architecture lens.
    Architecture,
    /// Quality attributes lens.
    Quality,
    /// Infrastructure / environment lens.
    Infrastructure,
    /// Delivery / handoff lens.
    Delivery,
}

/// Applicability and summary/rationale entry for a planning design lens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardPlanLensEntry {
    /// Lens identifier.
    pub lens: WorkflowBoardPlanLens,
    /// Applicability flag (`None` while undecided).
    pub applicable: Option<bool>,
    /// Summary required when `applicable == Some(true)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Reason required when `applicable == Some(false)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl<'de> Deserialize<'de> for WorkflowBoardPlanLensEntry {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Raw {
            lens: WorkflowBoardPlanLens,
            applicable: Option<bool>,
            #[serde(default)]
            summary: Option<String>,
            #[serde(default)]
            reason: Option<String>,
        }
        let raw = Raw::deserialize(deserializer)?;
        let entry = Self {
            lens: raw.lens,
            applicable: raw.applicable,
            summary: raw.summary,
            reason: raw.reason,
        };
        validate_lenses_partial(std::slice::from_ref(&entry)).map_err(de::Error::custom)?;
        Ok(entry)
    }
}

/// One of two materially distinct architectural alternatives compared during Plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardPlanAlternative {
    /// Alternative identifier (e.g. `alt-a`).
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub id: String,
    /// Short title.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub title: String,
    /// Architectural approach summary.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub summary: String,
    /// Explicit tradeoff description.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub tradeoffs: String,
}

/// Planning decomposition strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkflowBoardPlanningStrategy {
    /// Outcomes-driven decomposition.
    Outcomes,
    /// Risk-first probe decomposition.
    RiskFirst,
    /// Expand-contract migration decomposition.
    ExpandContract,
    /// Custom operator-specified strategy with rationale.
    Custom,
}

/// Strategy-specific metadata required by `risk-first` or `expand-contract`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardStrategyMetadata {
    /// Primary technical uncertainty (`risk-first`).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_trimmed_non_empty"
    )]
    pub uncertainty: Option<String>,
    /// Probe task ID (`risk-first`).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_trimmed_non_empty"
    )]
    pub probe_task_id: Option<String>,
    /// Dependent task IDs gated behind probe (`risk-first`).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_trimmed_non_empty_vec"
    )]
    pub dependent_task_ids: Option<Vec<String>>,
    /// Compatibility window (`expand-contract`).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_trimmed_non_empty"
    )]
    pub compatibility_window: Option<String>,
    /// Transition step (`expand-contract`).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_trimmed_non_empty"
    )]
    pub transition_step: Option<String>,
    /// Retirement prerequisite (`expand-contract`).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_trimmed_non_empty"
    )]
    pub retirement_prereq: Option<String>,
    /// Compatibility task IDs.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_trimmed_non_empty_vec"
    )]
    pub compatibility_task_ids: Option<Vec<String>>,
    /// Transition task IDs.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_trimmed_non_empty_vec"
    )]
    pub transition_task_ids: Option<Vec<String>>,
    /// Retirement task IDs.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_trimmed_non_empty_vec"
    )]
    pub retirement_task_ids: Option<Vec<String>>,
    /// Prerequisite evidence artifact paths.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_artifact_key_vec"
    )]
    pub prerequisite_evidence_paths: Option<Vec<String>>,
    /// Forward-compatible extra fields.
    #[serde(flatten, default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, Value>,
}

/// Task decomposition granularity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowBoardTaskGranularity {
    /// Fewer, larger tasks.
    Coarse,
    /// Standard balanced tasks.
    Balanced,
    /// Granular fine-grained tasks.
    Fine,
}

pub(crate) fn validate_opt_lenses(
    lenses: Option<&[WorkflowBoardPlanLensEntry]>,
) -> Result<(), String> {
    if let Some(items) = lenses {
        validate_lenses_partial(items)?;
    }
    Ok(())
}

pub(crate) fn validate_opt_alternatives(
    alts: Option<&[WorkflowBoardPlanAlternative]>,
) -> Result<(), String> {
    if let Some(items) = alts {
        validate_alternatives(items)?;
    }
    Ok(())
}
