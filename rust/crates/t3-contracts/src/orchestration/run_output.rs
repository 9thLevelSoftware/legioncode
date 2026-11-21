//! Typed run-output schemas, payloads, and submission commands (`2770bf3c61` / #38874).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;

use crate::ids::{CommandId, RunId, ThreadId};

/// Maximum JSON character length allowed for a run's declared `outputSchema`.
pub const MAX_RUN_OUTPUT_SCHEMA_CHARS: usize = 32_000;

/// JSON Schema object attached to a turn request or run (`Record<string, Schema.Json>`).
pub type RunOutputSchema = Map<String, Value>;

/// Structured output data accepted for a run (`OrchestrationV2RunOutput`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunOutputPayload {
    /// JSON value matching the run's declared `outputSchema`.
    pub value: Value,
    /// ISO-8601 timestamp when the output was accepted.
    pub submitted_at: String,
}

/// Payload for the `run.output-submitted` domain event (`OrchestrationV2RunOutputSubmitted`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunOutputSubmitted {
    /// Target run ID.
    pub run_id: RunId,
    /// Submitted structured output payload.
    pub output: RunOutputPayload,
}

/// Server-internal command committing a validated run output (`run.output.submit`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmitRunOutputCommand {
    /// Idempotency command identifier.
    pub command_id: CommandId,
    /// Owning thread identifier.
    pub thread_id: ThreadId,
    /// Target run identifier.
    pub run_id: RunId,
    /// Validated JSON value matching the run's `outputSchema`.
    pub value: Value,
}

/// Error returned when a run output schema or submission fails basic structural validation.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RunOutputValidationError {
    /// Schema JSON exceeds [`MAX_RUN_OUTPUT_SCHEMA_CHARS`].
    #[error("The output schema is larger than {max} characters.", max = MAX_RUN_OUTPUT_SCHEMA_CHARS)]
    SchemaTooLarge,
    /// Provider slash command cannot also request structured output.
    #[error("A command cannot be asked for a result. Ask for it in a message.")]
    CommandCannotRequestOutput,
}

/// Checks whether a serialized `outputSchema` fits within [`MAX_RUN_OUTPUT_SCHEMA_CHARS`].
pub fn validate_run_output_schema_size(
    schema: &RunOutputSchema,
) -> Result<(), RunOutputValidationError> {
    let encoded = serde_json::to_string(schema).unwrap_or_default();
    if encoded.len() > MAX_RUN_OUTPUT_SCHEMA_CHARS {
        return Err(RunOutputValidationError::SchemaTooLarge);
    }
    Ok(())
}

/// Renders agent instructions appended when a turn owes structured output matching `schema`.
pub fn format_run_output_instructions(schema: &RunOutputSchema) -> String {
    let json_schema = serde_json::to_string(schema).unwrap_or_else(|_| "{}".to_owned());
    [
        "A program reads the result of this turn, not a person.",
        "When the work is done, call the `t3_submit_output` tool once with `json` set to JSON text matching this JSON Schema:",
        &json_schema,
        "If you could not finish, still submit, with the fields that say so. A rejected submission names what did not fit: correct it and submit again.",
    ]
    .join("\n")
}
