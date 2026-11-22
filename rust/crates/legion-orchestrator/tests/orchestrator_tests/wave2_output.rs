use std::error::Error;

use legion_contracts::ids::{CommandId, RunId, ThreadId};
use legion_contracts::orchestration::{
    DispatchMode, MessageDispatchCommand, OrchestrationCommand, Run, RunStatus,
    SubmitRunOutputCommand,
};
use legion_orchestrator::decider::DeciderError;
use legion_orchestrator::decider::output_ops::{
    UntilOutputStatus, evaluate_until_output_wait, pick_run_for_output_submission,
};
use legion_orchestrator::sink::CommitCommandInput;
use serde_json::{Map, Value, json};

use super::helpers::{
    TestResult, create_thread_cmd, make_run_completed_event, new_test_orchestrator,
    send_message_cmd,
};

fn schema_obj(raw: Value) -> Result<Map<String, Value>, Box<dyn Error>> {
    match raw {
        Value::Object(map) => Ok(map),
        _ => Err("expected JSON schema object".into()),
    }
}

fn dispatch_with_schema(
    cmd: &str,
    thread: &str,
    msg: &str,
    text: &str,
    schema: Map<String, Value>,
) -> Result<OrchestrationCommand, Box<dyn Error>> {
    let base = send_message_cmd(cmd, thread, msg, text, DispatchMode::StartImmediately)?;
    Ok(match base {
        OrchestrationCommand::MessageDispatch(m) => {
            OrchestrationCommand::MessageDispatch(MessageDispatchCommand {
                output_schema: Some(schema),
                ..m
            })
        }
        other => other,
    })
}

#[tokio::test]
async fn typed_run_output_rejects_commands_and_invalid_json_against_schema() -> TestResult {
    let orch = new_test_orchestrator()?;
    let t0 = "2026-11-21T00:00:00.000Z";
    orch.dispatch_command(&create_thread_cmd("cmd-c1", "t-out", "p-1", "Out")?, t0)
        .await?;

    let schema = schema_obj(json!({
        "type": "object",
        "required": ["verdict"],
        "additionalProperties": false,
        "properties": { "verdict": { "type": "string" } }
    }))?;

    let bad_cmd = dispatch_with_schema(
        "cmd-bad-cmd",
        "t-out",
        "msg-bad",
        "/compact",
        schema.clone(),
    )?;
    assert!(matches!(
        orch.dispatch_command(&bad_cmd, t0).await,
        Err(legion_orchestrator::OrchestratorError::Decider(
            DeciderError::RunOutputRejected(_)
        ))
    ));

    let asked_cmd = dispatch_with_schema("cmd-asked", "t-out", "msg-asked", "Produce", schema)?;
    orch.dispatch_command(&asked_cmd, t0).await?;

    let invalid_submit = SubmitRunOutputCommand {
        command_id: CommandId::new("cmd-sub-bad")?,
        thread_id: ThreadId::new("t-out")?,
        run_id: RunId::new("cmd-asked-run")?,
        value: json!({ "verdict": "pass", "extra": 1 }),
    };
    assert!(
        orch.dispatch_submit_run_output(&invalid_submit, t0)
            .await
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn owed_completed_run_output_persists_across_stale_updates_and_until_output() -> TestResult {
    let orch = new_test_orchestrator()?;
    let t0 = "2026-11-21T00:00:00.000Z";
    orch.dispatch_command(&create_thread_cmd("cmd-c2", "t-owes", "p-1", "Owes")?, t0)
        .await?;

    let schema = schema_obj(json!({
        "type": "object",
        "required": ["verdict"],
        "additionalProperties": false,
        "properties": { "verdict": { "type": "string" } }
    }))?;
    orch.dispatch_command(
        &dispatch_with_schema("cmd-asked", "t-owes", "msg-asked", "Produce", schema)?,
        t0,
    )
    .await?;

    let state = orch.load_thread_state(&ThreadId::new("t-owes")?)?;
    let asked_run = state.runs.first().ok_or("missing asked run")?.clone();
    assert_eq!(
        evaluate_until_output_wait(&asked_run),
        UntilOutputStatus::Pending
    );

    orch.sink().commit_command(&CommitCommandInput {
        command_id: CommandId::new("cmd-complete-1")?,
        command_type: "run.complete".to_owned(),
        actor_kind: "runtime".to_owned(),
        thread_id: ThreadId::new("t-owes")?,
        causation_event_id: None,
        correlation_id: None,
        events: vec![make_run_completed_event(
            "ev-complete-1",
            asked_run.clone(),
            t0,
        )?],
        outbox_effects: Vec::new(),
        now_iso: t0.to_owned(),
    })?;

    orch.dispatch_command(
        &send_message_cmd(
            "cmd-followup",
            "t-owes",
            "msg-follow",
            "Continue",
            DispatchMode::StartImmediately,
        )?,
        t0,
    )
    .await?;
    let owed_state = orch.load_thread_state(&ThreadId::new("t-owes")?)?;
    let picked = pick_run_for_output_submission(&owed_state.runs)?;
    assert_eq!(picked.id.as_str(), "cmd-asked-run");

    orch.dispatch_submit_run_output(
        &SubmitRunOutputCommand {
            command_id: CommandId::new("cmd-sub-ok")?,
            thread_id: ThreadId::new("t-owes")?,
            run_id: RunId::new("cmd-asked-run")?,
            value: json!({ "verdict": "pass" }),
        },
        t0,
    )
    .await?;

    orch.sink().commit_command(&CommitCommandInput {
        command_id: CommandId::new("cmd-stale-2")?,
        command_type: "run.stale".to_owned(),
        actor_kind: "runtime".to_owned(),
        thread_id: ThreadId::new("t-owes")?,
        causation_event_id: None,
        correlation_id: None,
        events: vec![make_run_completed_event("ev-stale-2", asked_run, t0)?],
        outbox_effects: Vec::new(),
        now_iso: t0.to_owned(),
    })?;

    let final_runs = orch.load_thread_state(&ThreadId::new("t-owes")?)?.runs;
    let first = final_runs.first().ok_or("missing final run")?;
    assert_eq!(
        first.output.as_ref().map(|o| &o.value),
        Some(&json!({ "verdict": "pass" }))
    );
    assert_eq!(
        evaluate_until_output_wait(first),
        UntilOutputStatus::ResolvedWithOutput
    );

    let plain_done = Run {
        status: RunStatus::Completed,
        output_schema: None,
        output: None,
        ..final_runs[1].clone()
    };
    assert_eq!(
        evaluate_until_output_wait(&plain_done),
        UntilOutputStatus::ResolvedWithoutSchema
    );
    Ok(())
}
