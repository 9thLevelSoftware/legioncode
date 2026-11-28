#![forbid(unsafe_code)]
//! Shared driver launch payload, non-blocking agent turn execution, and schema builder for board stage jobs.

use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::engine_agent::AgentStepOptions;
use crate::engine_replay::ReplayContext;
use crate::engine_step::{PassControl, PassOutcome};
use crate::traits::{AgentLaunch, AgentModel};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StageLaunchPayload {
    pub card_id: String,
    pub project_id: String,
    pub title: String,
    pub brief_markdown: String,
    pub brief_path: String,
    pub spec_path: String,
    pub workspace_root: String,
    #[serde(default)]
    pub worktree_path: Option<String>,
    pub model_selection: AgentModel,
    pub runtime_mode: String,
    pub interaction_mode: String,
    pub workspace_strategy: Value,
    #[serde(default)]
    pub environment_id: Option<String>,
    pub stable_launch_key: String,
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub task_title: Option<String>,
    #[serde(default)]
    pub plan_path: Option<String>,
    #[serde(default)]
    pub files_allowed: Vec<String>,
    #[serde(default)]
    pub files_forbidden: Vec<String>,
    #[serde(default)]
    pub expected_artifacts: Vec<String>,
    #[serde(default)]
    pub verification_commands: Vec<Vec<String>>,
    #[serde(default)]
    pub product_snapshot_digest: Option<String>,
    #[serde(default)]
    pub review_id: Option<String>,
}

pub(crate) struct StageTurnSpec<'a> {
    pub stage_name: &'a str,
    pub step_id: &'a str,
    pub wait_key: &'a str,
    pub title_prefix: &'a str,
    pub lead_line: &'a str,
    pub spec_label: &'a str,
    pub brief_label: &'a str,
}

fn build_stage_context_extras(payload: &StageLaunchPayload) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    if let Some(ref tid) = payload.task_id {
        let title = payload.task_title.as_deref().unwrap_or("");
        let _ = writeln!(out, "- Active Task: `{tid}` {title}");
    }
    if let Some(ref plan_p) = payload.plan_path {
        let _ = writeln!(out, "- Approved plan file: `{plan_p}`");
    }
    if let Some(ref snap) = payload.product_snapshot_digest {
        let _ = writeln!(out, "- Product snapshot digest: `{snap}`");
    }
    if !payload.files_allowed.is_empty() {
        let _ = writeln!(
            out,
            "- Allowed files (`filesAllowed`): {}",
            payload.files_allowed.join(", ")
        );
    }
    if !payload.files_forbidden.is_empty() {
        let _ = writeln!(
            out,
            "- Forbidden files (`filesForbidden`): {}",
            payload.files_forbidden.join(", ")
        );
    }
    if !payload.expected_artifacts.is_empty() {
        let _ = writeln!(
            out,
            "- Expected artifacts: {}",
            payload.expected_artifacts.join(", ")
        );
    }
    if !payload.verification_commands.is_empty() {
        let cmds: Vec<String> = payload
            .verification_commands
            .iter()
            .map(|argv| argv.join(" "))
            .collect();
        let _ = writeln!(out, "- Verification commands: {}", cmds.join("; "));
    }
    if let Some(ref rid) = payload.review_id {
        let _ = writeln!(out, "- Review run id: `{rid}`");
    }
    out
}

pub(crate) async fn run_stage_turn_pass(
    ctx: &mut ReplayContext,
    spec: StageTurnSpec<'_>,
) -> PassOutcome {
    let StageTurnSpec {
        stage_name,
        step_id,
        wait_key,
        title_prefix,
        lead_line,
        spec_label,
        brief_label,
    } = spec;
    let payload: StageLaunchPayload = match serde_json::from_value(ctx.input.clone()) {
        Ok(parsed) => parsed,
        Err(err) => {
            return PassOutcome::Faulted(format!("Invalid {stage_name} payload: {err}"));
        }
    };

    let worktree_line = payload
        .worktree_path
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("(resolved at thread launch)");
    let context_extras = build_stage_context_extras(&payload);
    let prompt_title = match payload.task_id.as_deref() {
        Some(tid) => format!("{title_prefix} {tid}: {}", payload.title),
        None => format!("{title_prefix}: {}", payload.title),
    };
    let prompt = format!(
        "{lead_line} for workflow card `{card}`.\n\
         - Project workspace root: `{ws}`\n\
         - Active worktree: `{wt}`\n\
         - {spec_label}: `{spec}`\n\
         - {brief_label}: `{brief}`\n\
         {extras}\
         - Do not auto-commit `.legion/workflows/` files.\n\n\
         {md}",
        card = payload.card_id,
        ws = payload.workspace_root,
        wt = worktree_line,
        spec = payload.spec_path,
        brief = payload.brief_path,
        extras = context_extras,
        md = payload.brief_markdown,
    );

    let opts = AgentStepOptions {
        brief: prompt,
        title: Some(prompt_title),
        returns_schema: None,
        model: Some(payload.model_selection),
        project: Some(payload.project_id),
        environment: payload.environment_id,
        thread: None,
        issue: None,
        launch: Some(AgentLaunch {
            workspace: Some(payload.workspace_strategy),
            runtime_mode: Some(payload.runtime_mode),
            interaction_mode: Some(payload.interaction_mode),
            stable_launch_key: Some(payload.stable_launch_key),
        }),
        timeout_ms: None,
        quiet_ms: None,
    };

    let turn = match ctx.start_agent_turn(step_id, &opts).await {
        Ok(turn) => turn,
        Err(PassControl::Fault(reason)) => return PassOutcome::Faulted(reason),
        Err(PassControl::Suspend) => return PassOutcome::Suspended,
        Err(PassControl::Stop { skipped, reason }) => {
            return PassOutcome::Stopped { skipped, reason };
        }
    };

    match ctx.wait(wait_key, &Map::new(), None, None, None) {
        Ok(_) => PassOutcome::Completed(json!({
            "status": "completed",
            "cardId": payload.card_id,
            "threadId": turn.thread_id,
        })),
        Err(PassControl::Suspend) => PassOutcome::Suspended,
        Err(PassControl::Fault(reason)) => PassOutcome::Faulted(reason),
        Err(PassControl::Stop { skipped, reason }) => PassOutcome::Stopped { skipped, reason },
    }
}

pub(crate) fn stage_payload_schema() -> Map<String, Value> {
    let schema = json!({
        "type": "object",
        "properties": {
            "cardId": { "type": "string", "minLength": 1 },
            "projectId": { "type": "string", "minLength": 1 },
            "title": { "type": "string", "minLength": 1 },
            "briefMarkdown": { "type": "string", "minLength": 1 },
            "briefPath": { "type": "string", "minLength": 1 },
            "specPath": { "type": "string", "minLength": 1 },
            "workspaceRoot": { "type": "string", "minLength": 1 },
            "worktreePath": { "type": ["string", "null"] },
            "modelSelection": {
                "type": "object",
                "properties": {
                    "instanceId": { "type": "string", "minLength": 1 },
                    "model": { "type": "string", "minLength": 1 },
                    "options": {}
                },
                "required": ["instanceId", "model"]
            },
            "runtimeMode": { "type": "string", "minLength": 1 },
            "interactionMode": { "type": "string", "minLength": 1 },
            "workspaceStrategy": { "type": "object" },
            "environmentId": { "type": ["string", "null"] },
            "stableLaunchKey": { "type": "string", "minLength": 1 },
            "taskId": { "type": ["string", "null"] },
            "taskTitle": { "type": ["string", "null"] },
            "planPath": { "type": ["string", "null"] },
            "filesAllowed": { "type": "array", "items": { "type": "string" } },
            "filesForbidden": { "type": "array", "items": { "type": "string" } },
            "expectedArtifacts": { "type": "array", "items": { "type": "string" } },
            "verificationCommands": {
                "type": "array",
                "items": { "type": "array", "items": { "type": "string" } }
            },
            "productSnapshotDigest": { "type": ["string", "null"] },
            "reviewId": { "type": ["string", "null"] }
        },
        "required": [
            "cardId",
            "projectId",
            "title",
            "briefMarkdown",
            "briefPath",
            "specPath",
            "workspaceRoot",
            "modelSelection",
            "runtimeMode",
            "interactionMode",
            "workspaceStrategy",
            "stableLaunchKey"
        ]
    });
    schema.as_object().cloned().unwrap_or_default()
}
