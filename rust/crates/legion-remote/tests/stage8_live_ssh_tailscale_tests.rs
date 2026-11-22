#![forbid(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use legion_contracts::environment::{
    ExecutionEnvironmentDescriptor, PlatformArch, PlatformInfo, PlatformOs,
};
use legion_contracts::ids::EnvironmentId;
use legion_remote::live_network::{LiveSshError, LiveSshTunnelManager, LiveTailscaleController};
use legion_remote::tunnel::ssh::{BatchMode, SshTarget, SshTunnelState};
use legion_remote::tunnel::tailscale::{ServeTarget, TailscaleError};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::watch;

fn test_descriptor(
    id: &str,
    label: &str,
) -> Result<ExecutionEnvironmentDescriptor, Box<dyn std::error::Error>> {
    Ok(ExecutionEnvironmentDescriptor::builder(
        EnvironmentId::new(id)?,
        label,
        PlatformInfo::new(PlatformOs::Linux, PlatformArch::X64),
        "0.1.10",
    )
    .build())
}

async fn spawn_descriptor_http_server(
    mode: Arc<AtomicUsize>,
) -> Result<(u16, watch::Sender<bool>), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let (tx, mut rx) = watch::channel(false);
    let desc_a = serde_json::to_string(&test_descriptor("env-ssh-test", "Box A")?)?;
    let desc_b = serde_json::to_string(&test_descriptor("env-other-peer", "Other")?)?;

    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = rx.changed() => if *rx.borrow() { break; },
                accepted = listener.accept() => {
                    let Ok((mut stream, _)) = accepted else { break; };
                    let cur = mode.load(Ordering::Relaxed);
                    let (body, ct) = match cur {
                        1 => (desc_b.clone(), "application/json"),
                        2 => ("<html>not-legioncode</html>".to_owned(), "text/html"),
                        _ => (desc_a.clone(), "application/json"),
                    };
                    tokio::spawn(async move {
                        let mut buf = [0_u8; 2048];
                        let _ = stream.read(&mut buf).await;
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = stream.write_all(resp.as_bytes()).await;
                    });
                }
            }
        }
    });
    Ok((port, tx))
}

fn write_mock_ssh(dir: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    #[cfg(windows)]
    let (path, script) = (
        dir.join("mock-ssh.cmd"),
        "@echo off\r\nif \"%MOCK_SSH_MODE%\"==\"fail\" (\r\n  type \"%MOCK_SSH_ERR%\" 1>&2\r\n  exit /b 1\r\n)\r\nping -n 30 127.0.0.1 >nul 2>nul\r\n",
    );
    #[cfg(not(windows))]
    let (path, script) = (
        dir.join("mock-ssh.sh"),
        "#!/bin/sh\nif [ \"$MOCK_SSH_MODE\" = \"fail\" ]; then\n  cat \"$MOCK_SSH_ERR\" >&2\n  exit 1\nfi\nsleep 30\n",
    );
    fs::write(&path, script)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
    }
    Ok(path)
}

fn write_mock_tailscale(dir: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    #[cfg(windows)]
    let (path, script) = (
        dir.join("mock-tailscale.cmd"),
        "@echo off\r\nif \"%~1\"==\"status\" (\r\n  type \"%MOCK_TS_STATUS%\"\r\n  exit /b 0\r\n)\r\nif \"%~1\"==\"serve\" if \"%~2\"==\"status\" (\r\n  type \"%MOCK_TS_SERVE_STATUS%\"\r\n  exit /b 0\r\n)\r\nif \"%~1\"==\"serve\" (\r\n  if exist \"%MOCK_TS_SERVE_ERR%\" type \"%MOCK_TS_SERVE_ERR%\" 1>&2\r\n  exit /b %MOCK_TS_SERVE_EXIT%\r\n)\r\nexit /b 1\r\n",
    );
    #[cfg(not(windows))]
    let (path, script) = (
        dir.join("mock-tailscale.sh"),
        "#!/bin/sh\nif [ \"$1\" = \"status\" ]; then\n  cat \"$MOCK_TS_STATUS\"\n  exit 0\nfi\nif [ \"$1\" = \"serve\" ] && [ \"$2\" = \"status\" ]; then\n  cat \"$MOCK_TS_SERVE_STATUS\"\n  exit 0\nfi\nif [ \"$1\" = \"serve\" ]; then\n  [ -f \"$MOCK_TS_SERVE_ERR\" ] && cat \"$MOCK_TS_SERVE_ERR\" >&2\n  exit \"${MOCK_TS_SERVE_EXIT:-0}\"\nfi\nexit 1\n",
    );
    fs::write(&path, script)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
    }
    Ok(path)
}

fn mock_ts_ctl(
    mock_ts: &Path,
    status_file: &Path,
    serve_status_file: &Path,
    serve_err_file: &Path,
    serve_exit: &str,
) -> Result<LiveTailscaleController, Box<dyn std::error::Error>> {
    Ok(LiveTailscaleController::new()?
        .with_executable(mock_ts.to_string_lossy().into_owned())
        .with_env("MOCK_TS_STATUS", status_file.to_string_lossy().into_owned())
        .with_env(
            "MOCK_TS_SERVE_STATUS",
            serve_status_file.to_string_lossy().into_owned(),
        )
        .with_env(
            "MOCK_TS_SERVE_ERR",
            serve_err_file.to_string_lossy().into_owned(),
        )
        .with_env("MOCK_TS_SERVE_EXIT", serve_exit))
}

#[tokio::test]
async fn live_ssh_tunnel_manager_spawns_probes_redacts_stderr_and_stops()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let mock_ssh = write_mock_ssh(temp.path())?;
    let err_file = temp.path().join("ssh_err.txt");
    fs::write(&err_file, "fatal: {\"token\":\"top-secret-ssh-tok\"}\n")?;

    let target = SshTarget {
        alias: "devbox".to_owned(),
        hostname: "devbox.example.com".to_owned(),
        username: Some("alice".to_owned()),
        port: Some(2222),
    };

    let failing_mgr = LiveSshTunnelManager::new()?
        .with_ssh_binary(mock_ssh.to_string_lossy().into_owned())
        .with_env("MOCK_SSH_MODE", "fail")
        .with_env("MOCK_SSH_ERR", err_file.to_string_lossy().into_owned());

    let err = failing_mgr
        .start_forward_and_wait(
            &target,
            38_101,
            3_773,
            BatchMode::Yes,
            Duration::from_secs(2),
        )
        .await
        .expect_err("exiting mock ssh must fail early");
    match err {
        LiveSshError::ExitedEarly { stderr_tail, .. } => {
            assert!(!stderr_tail.contains("top-secret-ssh-tok"));
            assert!(stderr_tail.contains("[redacted]"));
        }
        other => panic!("unexpected error variant: {other}"),
    }

    let (port, shutdown_tx) = spawn_descriptor_http_server(Arc::new(AtomicUsize::new(0))).await?;
    let happy_mgr = LiveSshTunnelManager::new()?
        .with_ssh_binary(mock_ssh.to_string_lossy().into_owned())
        .with_env("MOCK_SSH_MODE", "hold");

    let (handle, desc) = happy_mgr
        .start_forward_and_wait(&target, port, 3_773, BatchMode::No, Duration::from_secs(5))
        .await?;
    assert_eq!(desc.environment_id.as_str(), "env-ssh-test");
    assert_eq!(handle.endpoints().local_port, port);
    assert!(matches!(
        handle.state_machine().state(),
        SshTunnelState::Ready { .. }
    ));
    let _actions = handle.stop().await?;
    let _ = shutdown_tx.send(true);
    Ok(())
}

#[tokio::test]
async fn live_tailscale_controller_maps_cli_errors() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let mock_ts = write_mock_tailscale(temp.path())?;
    let status_file = temp.path().join("status.json");
    let serve_status_file = temp.path().join("serve_status.json");
    let serve_err_file = temp.path().join("serve_err.txt");
    let target = ServeTarget {
        host: "127.0.0.1".to_owned(),
        port: 3_773,
    };

    fs::write(&serve_status_file, "null")?;
    fs::write(
        &status_file,
        r#"{"BackendState":"Running","Self":{"CapMap":{"https":null}}}"#,
    )?;
    let ctl = mock_ts_ctl(
        &mock_ts,
        &status_file,
        &serve_status_file,
        &serve_err_file,
        "0",
    )?;
    assert_eq!(
        ctl.publish_serve(&target, 443, "env-ssh-test").await,
        Err(TailscaleError::MagicDnsNameMissing)
    );

    fs::write(
        &status_file,
        r#"{"BackendState":"Running","Self":{"DNSName":"box.tail.ts.net.","CapMap":{}}}"#,
    )?;
    assert_eq!(
        ctl.publish_serve(&target, 443, "env-ssh-test").await,
        Err(TailscaleError::TailscaleServeNotEnabled)
    );

    fs::write(
        &status_file,
        r#"{"BackendState":"Running","Self":{"DNSName":"box.tail.ts.net.","CapMap":{"https":null}}}"#,
    )?;
    fs::write(&serve_err_file, "permission denied: must be root\n")?;
    let err_ctl = mock_ts_ctl(
        &mock_ts,
        &status_file,
        &serve_status_file,
        &serve_err_file,
        "1",
    )?;
    assert_eq!(
        err_ctl.ensure_serve(&target, 443).await,
        Err(TailscaleError::TailscaleOperatorRequired)
    );

    fs::write(
        &serve_err_file,
        "this Tailscale version doesn't support connecting to remote destinations\n",
    )?;
    assert_eq!(
        err_ctl.ensure_serve(&target, 443).await,
        Err(TailscaleError::TailscaleServeTargetRefused {
            local_host: "127.0.0.1".to_owned(),
        })
    );
    Ok(())
}

#[tokio::test]
async fn live_tailscale_controller_probes_and_publishes_serve()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let mock_ts = write_mock_tailscale(temp.path())?;
    let status_file = temp.path().join("status.json");
    let serve_status_file = temp.path().join("serve_status.json");
    let serve_err_file = temp.path().join("serve_err.txt");
    let target = ServeTarget {
        host: "127.0.0.1".to_owned(),
        port: 3_773,
    };

    fs::write(
        &status_file,
        r#"{"BackendState":"Running","Self":{"DNSName":"box.tail.ts.net.","CapMap":{"https":null}}}"#,
    )?;
    let live_other = TcpListener::bind("127.0.0.1:0").await?;
    let other_port = live_other.local_addr()?.port();
    fs::write(
        &serve_status_file,
        format!(
            r#"{{"Web":{{"box.tail.ts.net:443":{{"Handlers":{{"/":{{"Proxy":"http://127.0.0.1:{other_port}"}}}}}}}}}}"#
        ),
    )?;
    let ctl = mock_ts_ctl(
        &mock_ts,
        &status_file,
        &serve_status_file,
        &serve_err_file,
        "0",
    )?;
    assert_eq!(
        ctl.publish_serve(&target, 443, "env-ssh-test").await,
        Err(TailscaleError::ServePortOccupied { serve_port: 443 })
    );
    drop(live_other);

    let mode = Arc::new(AtomicUsize::new(1));
    let (probe_port, shutdown_tx) = spawn_descriptor_http_server(Arc::clone(&mode)).await?;
    let probe_origin = format!("http://127.0.0.1:{probe_port}");
    assert_eq!(
        ctl.clone()
            .with_probe_origin_override(&probe_origin)
            .publish_serve(&target, 443, "env-ssh-test")
            .await,
        Err(TailscaleError::ServesOtherEnvironment { serve_port: 443 })
    );

    mode.store(2, Ordering::Relaxed);
    assert_eq!(
        ctl.clone()
            .with_probe_origin_override(&probe_origin)
            .publish_serve(&target, 443, "env-ssh-test")
            .await,
        Err(TailscaleError::ServePortOccupied { serve_port: 443 })
    );

    let unused_listener = TcpListener::bind("127.0.0.1:0").await?;
    let closed_port = unused_listener.local_addr()?.port();
    drop(unused_listener);
    fs::write(&serve_status_file, "null")?;
    let published = ctl
        .clone()
        .with_probe_origin_override(format!("http://127.0.0.1:{closed_port}"))
        .publish_serve(&target, 443, "env-ssh-test")
        .await
        .map_err(|e| format!("publish_serve failed: {e:?}"))?;
    assert_eq!(published, "https://box.tail.ts.net/");
    ctl.disable_serve(443, None).await?;
    let _ = shutdown_tx.send(true);
    Ok(())
}
