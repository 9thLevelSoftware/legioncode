//! Integration tests for SSH argv/config/discovery and tunnel state machine.

use std::fs;

use legion_remote::tunnel::ssh::{
    BatchMode, RemoteServerKind, SshError, SshHostSource, SshTarget, SshTunnelState,
    SshTunnelStateMachine, TunnelAction, TunnelEvent, TunnelTransport, base_ssh_args,
    build_control_master_cancel_args, build_control_master_check_args,
    build_control_master_tunnel_args, build_direct_tunnel_args, build_ssh_command_args,
    discover_ssh_hosts, extract_control_path, host_key_verification_message, parse_known_hosts,
    parse_ssh_config_line, parse_ssh_resolve_output, redact_ssh_error_output, remote_state_key,
    resolve_loopback_http_base_url, validate_archive_version,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn sample_target(alias: &str) -> SshTarget {
    SshTarget {
        alias: alias.to_owned(),
        hostname: "devbox.example.com".to_owned(),
        username: Some("julius".to_owned()),
        port: Some(2222),
    }
}

#[test]
fn ssh_golden_keys_and_command_argv() -> TestResult {
    assert_eq!(
        remote_state_key(&sample_target("devbox")),
        "711bc738002d72fd"
    );

    let option_shaped = SshTarget {
        alias: "-oProxyCommand=bad".to_owned(),
        hostname: "example.com".to_owned(),
        username: None,
        port: None,
    };
    let argv = build_ssh_command_args(&option_shaped, BatchMode::Yes, &[], &["uname", "-a"])?;
    let sep_idx = argv
        .iter()
        .position(|a| a == "--")
        .ok_or("missing '--' separator")?;
    assert_eq!(argv[sep_idx + 1], "-oProxyCommand=bad");
    assert_eq!(&argv[sep_idx + 2..], ["uname", "-a"]);

    let base = base_ssh_args(&sample_target("devbox"), BatchMode::Yes);
    assert!(base.contains(&"RemoteCommand=none".to_owned()));
    assert!(base.contains(&"RequestTTY=no".to_owned()));

    let direct = build_direct_tunnel_args(&sample_target("devbox"), 4000, 3773, BatchMode::No)?;
    assert!(direct.contains(&"4000:127.0.0.1:3773".to_owned()));
    assert!(direct.contains(&"ControlMaster=no".to_owned()));

    let cm_tunnel =
        build_control_master_tunnel_args(&sample_target("devbox"), "/tmp/cm.sock", 4000, 3773)?;
    assert!(cm_tunnel.contains(&"ControlPath=/tmp/cm.sock".to_owned()));
    assert!(cm_tunnel.contains(&"127.0.0.1:4000:127.0.0.1:3773".to_owned()));

    let cm_cancel =
        build_control_master_cancel_args(&sample_target("devbox"), "/tmp/cm.sock", 4000, 3773)?;
    assert!(cm_cancel.windows(2).any(|w| w == ["-O", "cancel"]));

    let cm_check = build_control_master_check_args(&sample_target("devbox"), "/tmp/cm.sock")?;
    assert!(cm_check.windows(2).any(|w| w == ["-O", "check"]));

    let resolved = parse_ssh_resolve_output(
        "box",
        "hostname 10.0.0.9\nhostname 10.0.0.99\nuser alice\nport 2200\ncontrolpath none\n",
    );
    assert_eq!(resolved.hostname, "10.0.0.9");
    assert_eq!(resolved.username.as_deref(), Some("alice"));
    assert_eq!(resolved.port, Some(2200));
    assert_eq!(extract_control_path("controlpath none\n"), None);
    assert_eq!(
        extract_control_path("controlpath ~/.ssh/cm-%C\n").as_deref(),
        Some("~/.ssh/cm-%C")
    );

    let redacted = redact_ssh_error_output(r#"{"access_token":"secret-abc"}"#, false);
    assert!(!redacted.contains("secret-abc"));
    assert!(redacted.contains("[redacted]"));

    let host_msg = host_key_verification_message(
        &option_shaped,
        "Host key verification failed.\nREMOTE HOST IDENTIFICATION HAS CHANGED!",
        false,
    )
    .ok_or("expected verification message")?;
    assert!(host_msg.contains("has changed"));
    assert!(host_msg.contains("ssh -- -oProxyCommand=bad"));
    let spaced = SshTarget {
        alias: "dev box".to_owned(),
        hostname: "example.com".to_owned(),
        username: None,
        port: None,
    };
    let win_msg = host_key_verification_message(&spaced, "Host key verification failed.", true)
        .ok_or("expected win msg")?;
    assert!(win_msg.contains("\"dev box\""));

    validate_archive_version("1.2.3-beta.1")?;
    assert!(matches!(
        validate_archive_version("1.2"),
        Err(SshError::InvalidArchiveVersion { .. })
    ));
    assert_eq!(
        resolve_loopback_http_base_url(" http://127.0.0.1:3773/ ")?,
        "http://127.0.0.1:3773/"
    );
    assert!(matches!(
        resolve_loopback_http_base_url("http://example.com:3773/"),
        Err(SshError::HttpBridge { .. })
    ));
    Ok(())
}

#[test]
fn ssh_config_discovery_include_utf8_and_known_hosts() -> TestResult {
    let quoted = parse_ssh_config_line("  Include = \"conf.d/*.conf\" # comment")
        .ok_or("expected parsed line")?;
    assert_eq!(quoted.directive, "include");
    assert_eq!(quoted.args, Some(vec!["conf.d/*.conf".to_owned()]));
    let bad = parse_ssh_config_line("IdentityFile \"unclosed").ok_or("expected directive")?;
    assert_eq!(bad.args, None);

    let dir = tempfile::tempdir()?;
    let ssh_dir = dir.path().join(".ssh");
    let conf_d = ssh_dir.join("conf.d");
    fs::create_dir_all(&conf_d)?;
    fs::write(
        ssh_dir.join("config"),
        "Include conf.d/*.conf\nHost app\n  HostName %h.internal\n",
    )?;
    fs::write(
        conf_d.join("01-prod.conf"),
        "Host prod\n  HostName prod.example.com\n  Port 2222\n",
    )?;
    fs::write(
        ssh_dir.join("known_hosts"),
        "@cert-authority ca.example.com ssh-ed25519 AAA\n\
         |1|hashed ssh-ed25519 AAA\n\
         [prod.example.com]:2222,[extra.example.com]:2200 ssh-ed25519 AAA\n",
    )?;

    let known = parse_known_hosts(&fs::read_to_string(ssh_dir.join("known_hosts"))?);
    assert!(known.iter().any(|k| k.hostname == "ca.example.com"));

    let hosts = discover_ssh_hosts(dir.path())?;
    let app = hosts
        .iter()
        .find(|h| h.alias == "app")
        .ok_or("missing app host")?;
    assert_eq!(app.hostname, "app.internal");
    assert_eq!(app.source, SshHostSource::SshConfig);

    // prod.example.com:2222 is already covered by config, whereas extra.example.com:2200 is new.
    assert!(
        !hosts
            .iter()
            .any(|h| h.source == SshHostSource::KnownHosts && h.alias == "prod.example.com")
    );
    let extra = hosts
        .iter()
        .find(|h| h.alias == "extra.example.com")
        .ok_or("missing extra host")?;
    assert_eq!(extra.port, Some(2200));
    assert_eq!(extra.source, SshHostSource::KnownHosts);
    Ok(())
}

#[test]
fn ssh_tunnel_state_machine_fallback_prompt_and_teardown() -> TestResult {
    let target = sample_target("devbox");
    let mut sm = SshTunnelStateMachine::new(BatchMode::No);
    let _ = sm.handle(TunnelEvent::BeginResolve(target.clone()))?;
    let _ = sm.handle(TunnelEvent::ResolveSucceeded(target))?;
    let _ = sm.handle(TunnelEvent::RemoteLaunched {
        remote_port: 3773,
        local_port: 4100,
        remote_server_kind: Some(RemoteServerKind::Managed),
    })?;

    // Auth failure with ControlMaster socket falls back cleanly first.
    let actions = sm.handle(TunnelEvent::AuthFailed {
        error_message: "Permission denied (publickey).".to_owned(),
        prompt_available: true,
        live_control_path: Some("/tmp/legioncode.sock".to_owned()),
    })?;
    assert!(matches!(
        &actions[0],
        TunnelAction::SpawnForward {
            transport: TunnelTransport::ControlMaster { control_path },
            ..
        } if control_path == "/tmp/legioncode.sock"
    ));

    // Subsequent auth failures prompt up to twice, then transition to Failed.
    let p1 = sm.handle(TunnelEvent::AuthFailed {
        error_message: "Permission denied (password).".to_owned(),
        prompt_available: true,
        live_control_path: None,
    })?;
    assert_eq!(p1, vec![TunnelAction::PromptPassword { attempt: 1 }]);
    let p2 = sm.handle(TunnelEvent::AuthFailed {
        error_message: "Permission denied (password).".to_owned(),
        prompt_available: true,
        live_control_path: None,
    })?;
    assert_eq!(p2, vec![TunnelAction::PromptPassword { attempt: 2 }]);

    let _ = sm.handle(TunnelEvent::ForwardListening)?;
    let _ = sm.handle(TunnelEvent::HttpReady)?;
    assert!(matches!(sm.state(), SshTunnelState::Ready { .. }));

    // Stale teardown MUST set stop_remote=false and cancel ControlMaster forward.
    let close_actions = sm.handle(TunnelEvent::StaleDetected)?;
    assert!(matches!(
        &close_actions[0],
        TunnelAction::CloseForward {
            control_cancel_args: Some(_),
            stop_remote: false,
        }
    ));
    let _ = sm.handle(TunnelEvent::ShutdownCompleted)?;
    assert_eq!(*sm.state(), SshTunnelState::Closed);

    // Invalid transition from Closed errors cleanly.
    assert!(matches!(
        sm.handle(TunnelEvent::Disconnect),
        Err(SshError::InvalidStateTransition { from: "Closed", .. })
    ));
    Ok(())
}
