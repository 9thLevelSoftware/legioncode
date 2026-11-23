//! Workflow definition files, loader state, and edit RPC contracts (`workflow.ts`).

use serde::{Deserialize, Serialize};

/// Parse, load, or validation diagnostic for workflow definition files (`WorkflowDefinitionProblem`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowDefinitionProblem {
    /// Diagnostic explanation.
    pub message: String,
    /// Offending definition filename, if scoped to one file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// 1-based line number if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<i64>,
}

/// Current definition-loader revision and error summary (`WorkflowDefinitionState`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowDefinitionState {
    /// Monotonic loader revision counter.
    pub revision: i64,
    /// Top-level load failure preventing replacement of running definitions, or `None`.
    pub problem: Option<String>,
}

/// Single workflow definition source file (`WorkflowDefinitionFile`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowDefinitionFile {
    /// Definition file name.
    pub name: String,
    /// On-disk source contents.
    pub source: String,
    /// Registered `<name>@<version>` entries from the last valid load.
    pub workflows: Vec<String>,
    /// File-level diagnostic problems.
    pub problems: Vec<WorkflowDefinitionProblem>,
}

/// Full definition workspace returned by `workflows.getDefinition` (`WorkflowDefinitionFiles`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowDefinitionFiles {
    /// Directory containing workflow definition files.
    pub directory: String,
    /// True when pointed at a single immutable filename.
    pub single: bool,
    /// Individual definition files.
    pub files: Vec<WorkflowDefinitionFile>,
    /// Cross-file diagnostic problems.
    pub problems: Vec<WorkflowDefinitionProblem>,
    /// Loader revision counter.
    pub revision: i64,
    /// Top-level loader error, or `None`.
    pub problem: Option<String>,
}

/// Empty payload for `workflows.getDefinition` (`WorkflowDefinitionGetInput`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowDefinitionGetInput {}

/// Input payload for `workflows.saveDefinition` (`WorkflowDefinitionSaveInput`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowDefinitionSaveInput {
    /// Definition file name inside the workflows directory.
    pub name: String,
    /// Full file source text.
    pub source: String,
}

/// Input payload for `workflows.deleteDefinition` (`WorkflowDefinitionDeleteInput`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowDefinitionDeleteInput {
    /// Definition file name inside the workflows directory.
    pub name: String,
}

/// Result returned by `workflows.saveDefinition` / `workflows.deleteDefinition` (`WorkflowDefinitionChangeResult`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowDefinitionChangeResult {
    /// True when the file on disk was modified or removed.
    pub changed: bool,
    /// Resulting validation or compilation diagnostics.
    pub problems: Vec<WorkflowDefinitionProblem>,
}
