use legion_contracts::orchestration::{
    INTERNAL_COMMAND_TYPES, OrchestrationCommand, PatchValue, ProjectWireCommand,
    StoredDomainEvent, StreamItem, WireClientCommand, decode_wire_client_command,
    decode_wire_orchestration_command, is_internal_command_type, is_known_domain_event_type,
    is_known_turn_item_type,
};
use proptest::prelude::*;
use serde_json::{Value, json};

#[test]
fn client_commands_accept_valid_variants_and_reject_server_internal_commands() {
    let create_cmd = json!({
        "type": "thread.create",
        "createdBy": "user",
        "creationSource": "web",
        "commandId": "cmd-1",
        "threadId": "thread-1",
        "projectId": "proj-1",
        "title": "New thread",
        "modelSelection": { "provider": "codex-main", "model": "gpt-5" },
        "runtimeMode": "approval-required",
        "interactionMode": "default",
        "branch": null,
        "worktreePath": null
    });
    let cmd: OrchestrationCommand = serde_json::from_value(create_cmd).expect("thread.create");
    assert_eq!(cmd.command_type(), "thread.create");
    assert_eq!(cmd.command_id().as_str(), "cmd-1");

    let patch_cmd: OrchestrationCommand = serde_json::from_value(json!({
        "type": "thread.metadata.update",
        "commandId": "cmd-2",
        "threadId": "thread-1",
        "branch": null
    }))
    .expect("metadata patch");
    if let OrchestrationCommand::ThreadMetadataUpdate(meta) = patch_cmd {
        assert_eq!(meta.branch, PatchValue::Null);
        assert!(meta.title.is_none());
    } else {
        panic!("expected ThreadMetadataUpdate");
    }

    assert_eq!(INTERNAL_COMMAND_TYPES.len(), 17);
    assert!(is_internal_command_type("thread.guest-share.open"));
    assert!(is_internal_command_type("feedback_request.record"));
    for internal_type in INTERNAL_COMMAND_TYPES {
        let raw = json!({
            "type": internal_type,
            "commandId": "cmd-int",
            "threadId": "thread-1"
        });
        assert!(
            serde_json::from_value::<OrchestrationCommand>(raw).is_err(),
            "internal command `{internal_type}` must be rejected"
        );
    }
}

#[test]
fn stored_domain_event_round_trips_thread_created_with_defaulted_fields() {
    let raw = json!({
        "sequence": 1,
        "commandId": "cmd-1",
        "event": {
            "id": "ev-1",
            "threadId": "t-1",
            "occurredAt": "2026-04-01T00:00:00Z",
            "type": "thread.created",
            "payload": {
                "createdBy": "user",
                "creationSource": "web",
                "id": "t-1",
                "projectId": "p-1",
                "title": "Test",
                "providerInstanceId": "inst-1",
                "modelSelection": { "instanceId": "inst-1", "model": "claude-haiku-4-5" },
                "runtimeMode": "full-access",
                "interactionMode": "default",
                "branch": "main",
                "worktreePath": null,
                "activeProviderThreadId": null,
                "lineage": {
                    "parentThreadId": null,
                    "relationshipToParent": null,
                    "rootThreadId": "t-1"
                },
                "forkedFrom": null,
                "createdAt": "2026-04-01T00:00:00Z",
                "updatedAt": "2026-04-01T00:00:00Z",
                "archivedAt": null,
                "deletedAt": null
            }
        }
    });

    let stored: StoredDomainEvent = serde_json::from_value(raw).expect("stored domain event");
    assert_eq!(stored.sequence, 1);
    assert_eq!(stored.event.payload.event_type(), "thread.created");
    let reencoded = serde_json::to_string(&stored).expect("serialize stored event");
    let reparsed: StoredDomainEvent =
        serde_json::from_str(&reencoded).expect("reparse stored event");
    assert_eq!(reparsed, stored);
}

#[test]
fn stream_item_bug18_rejects_non_string_tags_and_maps_unknown_strings() {
    assert!(is_known_domain_event_type("turn-item.updated"));
    assert!(is_known_turn_item_type("assistant_message"));

    let sync: StreamItem = serde_json::from_value(json!({ "kind": "synchronized" })).expect("sync");
    assert_eq!(sync, StreamItem::Synchronized);

    let unknown_event: StreamItem = serde_json::from_value(json!({
        "kind": "event",
        "sequence": 42,
        "event": {
            "id": "e42",
            "type": "future.v3.metric",
            "threadId": "t1",
            "occurredAt": "2026-04-01T00:00:00Z",
            "payload": {}
        }
    }))
    .expect("unknown string event maps cleanly");
    assert_eq!(
        unknown_event,
        StreamItem::UnknownEvent {
            sequence: 42,
            event_type: "future.v3.metric".to_owned(),
        }
    );

    let unknown_turn_item: StreamItem = serde_json::from_value(json!({
        "kind": "event",
        "sequence": 43,
        "event": {
            "id": "e43",
            "type": "turn-item.updated",
            "threadId": "t1",
            "occurredAt": "2026-04-01T00:00:00Z",
            "payload": {
                "type": "future_widget_item",
                "id": "ti-1"
            }
        }
    }))
    .expect("unknown string turn item maps cleanly");
    assert_eq!(
        unknown_turn_item,
        StreamItem::UnknownEvent {
            sequence: 43,
            event_type: "turn-item.updated".to_owned(),
        }
    );
}

fn non_string_tag_strategy() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(|n| Value::Number(n.into())),
        Just(json!([])),
        Just(json!({})),
    ]
}

proptest! {
    #[test]
    fn bug18_non_string_tags_always_fail_stream_deserialization(bad_tag in non_string_tag_strategy()) {
        let bad_event = json!({
            "kind": "event",
            "sequence": 1,
            "event": {
                "id": "e1",
                "type": bad_tag,
                "threadId": "t1",
                "occurredAt": "2026-04-01T00:00:00Z",
                "payload": {}
            }
        });
        prop_assert!(serde_json::from_value::<StreamItem>(bad_event).is_err());

        let bad_turn_item = json!({
            "kind": "event",
            "sequence": 2,
            "event": {
                "id": "e2",
                "type": "turn-item.updated",
                "threadId": "t1",
                "occurredAt": "2026-04-01T00:00:00Z",
                "payload": {
                    "type": bad_tag,
                    "id": "ti-1"
                }
            }
        });
        prop_assert!(serde_json::from_value::<StreamItem>(bad_turn_item).is_err());
    }
}

#[test]
fn wire_command_decoders_handle_v1_aliases_projects_and_reject_internal() {
    let start_v1 = decode_wire_orchestration_command(json!({
        "type": "thread.turn.start",
        "commandId": "cmd-start-1",
        "threadId": "t-1",
        "message": { "messageId": "msg-v1", "text": "hello world", "attachments": [] }
    }))
    .expect("v1 turn start");
    assert_eq!(start_v1.command_type(), "message.dispatch");

    let interrupt_with_turn = decode_wire_orchestration_command(json!({
        "type": "thread.turn.interrupt",
        "commandId": "cmd-int-1",
        "threadId": "t-1",
        "turnId": "run-1"
    }))
    .expect("v1 turn interrupt with turnId");
    assert_eq!(interrupt_with_turn.command_type(), "run.interrupt");

    let interrupt_stop = decode_wire_orchestration_command(json!({
        "type": "thread.turn.interrupt",
        "commandId": "cmd-int-2",
        "threadId": "t-1"
    }))
    .expect("v1 turn interrupt without turnId");
    assert_eq!(interrupt_stop.command_type(), "thread.stop");

    let approval = decode_wire_orchestration_command(json!({
        "type": "thread.approval.respond",
        "commandId": "cmd-app-1",
        "threadId": "t-1",
        "requestId": "req-1",
        "decision": "accept"
    }))
    .expect("v1 approval respond");
    assert_eq!(approval.command_type(), "runtime-request.respond");

    let user_input = decode_wire_orchestration_command(json!({
        "type": "thread.user-input.respond",
        "commandId": "cmd-ui-1",
        "threadId": "t-1",
        "requestId": "req-2",
        "answers": { "q1": "yes" }
    }))
    .expect("v1 user input respond");
    assert_eq!(user_input.command_type(), "runtime-request.respond");

    let revert = decode_wire_orchestration_command(json!({
        "type": "thread.checkpoint.revert",
        "commandId": "cmd-rev-1",
        "threadId": "t-1",
        "turnCount": 2
    }))
    .expect("v1 checkpoint revert");
    assert_eq!(revert.command_type(), "checkpoint.rollback");

    let project_create = decode_wire_client_command(json!({
        "type": "project.create",
        "commandId": "cmd-pc-1",
        "title": "Workspace",
        "workspaceRoot": "/repo",
        "defaultModelSelection": { "provider": "claude", "model": "claude-haiku-4-5" }
    }))
    .expect("project.create");
    match project_create {
        WireClientCommand::Project(proj) => match *proj {
            ProjectWireCommand::Create(create) => {
                assert_eq!(create.title, "Workspace");
                assert!(matches!(create.default_model_selection, PatchValue::Set(_)));
            }
            other => panic!("expected ProjectWireCommand::Create, got {other:?}"),
        },
        other @ WireClientCommand::Orchestration(_) => {
            panic!("expected WireClientCommand::Project, got {other:?}")
        }
    }

    let project_update = decode_wire_client_command(json!({
        "type": "project.update",
        "projectId": "p-1",
        "autoPull": true
    }))
    .expect("project.update alias");
    assert!(matches!(
        project_update,
        WireClientCommand::Project(ref p) if matches!(**p, ProjectWireCommand::Update(_))
    ));

    let rejected = decode_wire_client_command(json!({
        "type": "thread.guest-share.open",
        "commandId": "cmd-bad",
        "threadId": "t-1"
    }));
    assert!(rejected.is_err());
}
