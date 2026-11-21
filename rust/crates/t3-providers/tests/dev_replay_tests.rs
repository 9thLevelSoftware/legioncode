//! Deterministic `Replay (dev)` turn-stream and ACP round-trip tests.

use t3_providers::acp::{AcpStdioCodec, SessionUpdateKind};
use t3_providers::drivers::{DevReplayScenario, generate_dev_replay_events};

#[test]
fn dev_replay_generates_deterministic_roundtrip_acp_events()
-> Result<(), Box<dyn std::error::Error>> {
    let first = generate_dev_replay_events("s-dev", "/repo/", DevReplayScenario::Showcase, 0);
    let second = generate_dev_replay_events("s-dev", "/repo/", DevReplayScenario::Showcase, 0);
    assert_eq!(first, second);
    assert!(
        first
            .iter()
            .any(|e| matches!(e.update, SessionUpdateKind::ToolCall(_)))
    );

    let mut codec = AcpStdioCodec::new();
    for notif in &first {
        let line = codec.encode_notification("session/update", Some(notif))?;
        let envelopes = codec.push_bytes(line.as_bytes())?;
        assert_eq!(envelopes.len(), 1);
    }

    let todo = generate_dev_replay_events("s-dev", "/repo", DevReplayScenario::TodoList, 1);
    assert!(
        todo.iter()
            .any(|e| matches!(e.update, SessionUpdateKind::Plan { .. }))
    );
    Ok(())
}

#[test]
fn dev_replay_emits_non_empty_events_across_all_scenarios() {
    for scenario in [
        DevReplayScenario::Showcase,
        DevReplayScenario::TodoList,
        DevReplayScenario::Approval,
        DevReplayScenario::Question,
        DevReplayScenario::MultiTurn,
        DevReplayScenario::CommandOutput,
    ] {
        let events = generate_dev_replay_events("s-all", "/workspace", scenario, 2);
        assert_ne!(events.len(), 0);
    }
}
