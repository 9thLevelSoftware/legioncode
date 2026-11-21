//! Git RPC payload validation rules for `git.preparePullRequestThread` and `git.runStackedAction`.

use serde_json::{Map, Value};

use super::{EnvironmentScope, GuardEvalError};

pub(super) fn validate_trimmed_non_empty_field(
    obj: &Map<String, Value>,
    field: &str,
    method: &str,
) -> Result<String, GuardEvalError> {
    let Some(raw) = obj.get(field).and_then(Value::as_str) else {
        return Err(GuardEvalError::InvalidInput {
            method: method.to_owned(),
            reason: format!("missing or non-string field `{field}`"),
        });
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(GuardEvalError::InvalidInput {
            method: method.to_owned(),
            reason: format!("field `{field}` cannot be empty"),
        });
    }
    Ok(trimmed.to_owned())
}

pub(super) fn validate_optional_trimmed_id(
    obj: &Map<String, Value>,
    field: &str,
    method: &str,
) -> Result<Option<String>, GuardEvalError> {
    let Some(raw_val) = obj.get(field) else {
        return Ok(None);
    };
    let Some(raw) = raw_val.as_str() else {
        return Err(GuardEvalError::InvalidInput {
            method: method.to_owned(),
            reason: format!("optional field `{field}` must be a non-empty string when present"),
        });
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(GuardEvalError::InvalidInput {
            method: method.to_owned(),
            reason: format!("optional field `{field}` cannot be empty"),
        });
    }
    Ok(Some(trimmed.to_owned()))
}

pub(super) fn eval_git_prepare_pr_thread(
    input: &Value,
) -> Result<Vec<EnvironmentScope>, GuardEvalError> {
    const METHOD: &str = "git.preparePullRequestThread";
    let Some(obj) = input.as_object() else {
        return Err(GuardEvalError::InvalidInput {
            method: METHOD.to_owned(),
            reason: "expected JSON object payload".to_owned(),
        });
    };
    validate_trimmed_non_empty_field(obj, "cwd", METHOD)?;
    validate_trimmed_non_empty_field(obj, "reference", METHOD)?;
    let mode = validate_trimmed_non_empty_field(obj, "mode", METHOD)?;
    if mode != "local" && mode != "worktree" {
        return Err(GuardEvalError::InvalidInput {
            method: METHOD.to_owned(),
            reason: format!("invalid `mode` `{mode}` (expected `local` or `worktree`)"),
        });
    }
    let thread_id = validate_optional_trimmed_id(obj, "threadId", METHOD)?;
    if mode == "worktree" && thread_id.is_some() {
        Ok(vec![
            EnvironmentScope::SourceControlWrite,
            EnvironmentScope::OrchestrationOperate,
        ])
    } else {
        Ok(vec![EnvironmentScope::SourceControlWrite])
    }
}

pub(super) fn eval_git_run_stacked_action(
    input: &Value,
) -> Result<Vec<EnvironmentScope>, GuardEvalError> {
    const METHOD: &str = "git.runStackedAction";
    let Some(obj) = input.as_object() else {
        return Err(GuardEvalError::InvalidInput {
            method: METHOD.to_owned(),
            reason: "expected JSON object payload".to_owned(),
        });
    };
    validate_trimmed_non_empty_field(obj, "actionId", METHOD)?;
    validate_trimmed_non_empty_field(obj, "cwd", METHOD)?;
    let action = validate_trimmed_non_empty_field(obj, "action", METHOD)?;
    if !matches!(
        action.as_str(),
        "commit" | "push" | "create_pr" | "commit_push" | "commit_push_pr"
    ) {
        return Err(GuardEvalError::InvalidInput {
            method: METHOD.to_owned(),
            reason: format!("invalid `action` `{action}`"),
        });
    }
    let thread_id = validate_optional_trimmed_id(obj, "threadId", METHOD)?;
    if thread_id.is_some() && (action == "create_pr" || action == "commit_push_pr") {
        Ok(vec![
            EnvironmentScope::SourceControlWrite,
            EnvironmentScope::OrchestrationRead,
        ])
    } else {
        Ok(vec![EnvironmentScope::SourceControlWrite])
    }
}
