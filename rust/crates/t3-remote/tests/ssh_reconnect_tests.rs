//! Integration tests for SSH forward resilience (`#39802`, `#39807`).

use std::time::Duration;

use t3_contracts::brand::LEGION_CODE;
use t3_remote::ssh_helper::{WaitReadyOutcome, wait_ready_outcome_async};
use t3_remote::tunnel::ssh::{
    BatchMode, ExistingForwardDecision, ExistingForwardProbe, ForwardHttpOutcome,
    RemoteReconnectDecision, SshTarget, SshTunnelState, SshTunnelStateMachine, TunnelEvent,
    evaluate_existing_forward, evaluate_remote_stopping_reconnect_for_brand,
    probe_existing_forward_async,
};
use tokio::net::TcpListener;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[tokio::test]
async fn ssh_forward_resilience_across_slow_handshake_and_clean_shutdown() -> TestResult {
    // #39802: Keep slow forward open when timeout without answer while listener+process active.
    assert_eq!(
        evaluate_existing_forward(ExistingForwardProbe {
            http_outcome: ForwardHttpOutcome::TimedOut,
            process_running: true,
            local_listener_active: true,
        }),
        ExistingForwardDecision::KeepSlowForward
    );
    let slow_listener = TcpListener::bind("127.0.0.1:0").await?;
    let slow_port = slow_listener.local_addr()?.port();
    let _accept_hold = tokio::spawn(async move {
        let (_conn, _) = slow_listener.accept().await.ok()?;
        tokio::time::sleep(Duration::from_secs(5)).await;
        Some(())
    });
    assert_eq!(
        probe_existing_forward_async(
            &format!("http://127.0.0.1:{slow_port}/"),
            slow_port,
            Duration::from_millis(80),
            true
        )
        .await,
        ExistingForwardDecision::KeepSlowForward
    );

    // #39807: Closed port yields RefusedOnly (exit code 2) -> StopWhileShuttingDown when PID still alive.
    // Give Windows Winsock >=2500ms probe timeout so WSAECONNREFUSED completes before reqwest timeout.
    let dead_port = {
        let l = TcpListener::bind("127.0.0.1:0").await?;
        l.local_addr()?.port()
    };
    let outcome = wait_ready_outcome_async(dead_port, 3_000, 2_600).await;
    assert_eq!(outcome, WaitReadyOutcome::RefusedOnly);
    assert_eq!(outcome.exit_code(), 2);
    let decision =
        evaluate_remote_stopping_reconnect_for_brand(Some(99), 99, outcome, true, &LEGION_CODE);
    assert!(matches!(
        decision,
        RemoteReconnectDecision::StopWhileShuttingDown { pid: 99, .. }
    ));

    let mut sm = SshTunnelStateMachine::new(BatchMode::No);
    let _ = sm.handle(TunnelEvent::BeginResolve(SshTarget {
        alias: "box".to_owned(),
        hostname: "box.example.com".to_owned(),
        username: None,
        port: None,
    }))?;
    let actions = sm.handle(TunnelEvent::RemoteShuttingDown { pid: 99 })?;
    assert_eq!(actions, vec![]);
    assert!(
        matches!(sm.state(), SshTunnelState::Failed { message, .. } if message.contains("PID 99"))
    );
    Ok(())
}
