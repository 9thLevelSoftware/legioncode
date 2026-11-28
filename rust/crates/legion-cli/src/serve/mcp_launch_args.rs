//! Argument parsing helpers for `/mcp` `legion_thread_launch` and `legion_submit_output`.

use legion_contracts::ids::EnvironmentId;
use legion_contracts::orchestration::{
    ModelSelection, ProviderInteractionMode, RunOutputSchema, WorkflowOrigin, WorkspaceStrategy,
};
use legion_contracts::policy::RuntimeMode;
use legion_persistence::project_store::ProjectRecord;
use legion_server_runtime::{
    parse_interaction_mode_strict, parse_runtime_mode_strict, resolve_default_model_selection,
    resolve_default_runtime_mode,
};
use serde_json::Value;

use super::super::AppState;

/// Parses an optional `outputSchema` JSON object from `/mcp` tool arguments.
pub(crate) fn parse_output_schema_arg(args: &Value) -> Result<Option<RunOutputSchema>, String> {
    let Some(raw) = args.get("outputSchema") else {
        return Ok(None);
    };
    if raw.is_null() {
        return Ok(None);
    }
    let Some(map) = raw.as_object() else {
        return Err("outputSchema must be a JSON Schema object.".to_owned());
    };
    Ok(Some(map.clone()))
}

pub(super) fn parse_or_resolve_model_selection(
    raw: Option<&Value>,
    project: &ProjectRecord,
    state: &AppState,
) -> Result<ModelSelection, String> {
    if let Some(val) = raw
        && !val.is_null()
    {
        return serde_json::from_value::<ModelSelection>(val.clone())
            .map_err(|e| format!("Invalid modelSelection: {e}"));
    }
    resolve_default_model_selection(project, Some(&state.paths.settings_path))
        .map_err(|e| e.to_string())
}

pub(super) fn parse_or_resolve_runtime_mode(
    raw: Option<&Value>,
    state: &AppState,
) -> Result<RuntimeMode, String> {
    match raw {
        Some(val) if !val.is_null() => {
            let s = val
                .as_str()
                .ok_or_else(|| "runtimeMode must be a string.".to_owned())?;
            parse_runtime_mode_strict(s).map_err(|e| e.to_string())
        }
        _ => resolve_default_runtime_mode(Some(&state.paths.settings_path))
            .map_err(|e| e.to_string()),
    }
}

pub(super) fn parse_interaction_mode(
    raw: Option<&Value>,
) -> Result<ProviderInteractionMode, String> {
    match raw {
        Some(val) if !val.is_null() => {
            let s = val
                .as_str()
                .ok_or_else(|| "interactionMode must be a string.".to_owned())?;
            parse_interaction_mode_strict(s).map_err(|e| e.to_string())
        }
        _ => Ok(ProviderInteractionMode::Default),
    }
}

pub(super) fn parse_workspace_strategy(raw: Option<&Value>) -> Result<WorkspaceStrategy, String> {
    match raw {
        Some(val) if !val.is_null() => serde_json::from_value::<WorkspaceStrategy>(val.clone())
            .map_err(|e| format!("Invalid workspaceStrategy: {e}")),
        _ => Ok(WorkspaceStrategy::Root { branch: None }),
    }
}

pub(super) fn parse_workflow_run(
    raw: Option<&Value>,
    default_env_id: &EnvironmentId,
) -> Result<Option<WorkflowOrigin>, String> {
    let Some(val) = raw else {
        return Ok(None);
    };
    if val.is_null() {
        return Ok(None);
    }
    let Some(obj) = val.as_object() else {
        return Err("workflowRun must be a JSON object.".to_owned());
    };
    let run_id = obj
        .get("runId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "workflowRun.runId must be a non-empty string.".to_owned())?
        .to_owned();
    let workflow = obj
        .get("workflow")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "workflowRun.workflow must be a non-empty string.".to_owned())?
        .to_owned();
    let environment_id = match obj
        .get("environmentId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(raw_env) => EnvironmentId::new(raw_env).map_err(|e| e.to_string())?,
        None => default_env_id.clone(),
    };
    let step = obj
        .get("step")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("mcp")
        .to_owned();
    Ok(Some(WorkflowOrigin {
        environment_id,
        run_id,
        workflow,
        step,
    }))
}
