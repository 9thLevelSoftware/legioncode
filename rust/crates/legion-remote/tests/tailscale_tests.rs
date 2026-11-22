//! Integration tests for Tailscale CLI status/serve JSON parsing and pairing errors.

use std::path::Path;

use legion_remote::tunnel::tailscale::{
    DEFAULT_TAILSCALE_SERVE_PORT, ServeTarget, TailscaleError, TailscaleServeRootHolder,
    TailscaleServeRootStatus, TailscaleStderrDiagnostic, build_tailscale_https_base_url,
    classify_serve_root_holder, classify_stderr_diagnostic, ensure_tailscale_serve_args,
    parse_tailscale_exit_node, parse_tailscale_serve_root_holders, parse_tailscale_serve_routes_to,
    parse_tailscale_ssh_enabled, parse_tailscale_status, tailscale_serve_off_args,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn tailscale_status_url_and_exact_pair_errors() -> TestResult {
    assert_eq!(DEFAULT_TAILSCALE_SERVE_PORT, 443);
    let status = parse_tailscale_status(
        r#"{"BackendState":"Running","Self":{"DNSName":"My-Box.tail123.ts.net.","TailscaleIPs":["100.100.1.2","fd7a:115c:a1e0::1","192.168.1.5"],"CapMap":{"https":null}}}"#,
    )?;
    assert_eq!(status.running, Some(true));
    assert_eq!(
        status.magic_dns_name.as_deref(),
        Some("My-Box.tail123.ts.net")
    );
    assert_eq!(status.tailnet_ipv4_addresses, vec!["100.100.1.2"]);
    assert_eq!(status.https_serve_enabled, Some(true));

    let exit = parse_tailscale_exit_node(
        r#"{"ExitNodeStatus":{"Online":true},"Peer":{"n1":{"ExitNode":true,"DNSName":"exit.tail.ts.net."}}}"#,
    )?;
    assert_eq!(exit.and_then(|e| e.name).as_deref(), Some("exit"));
    assert!(parse_tailscale_ssh_enabled(
        r#"{"RunSSH":true,"WantRunning":true}"#
    )?);
    assert!(!parse_tailscale_ssh_enabled(
        r#"{"RunSSH":true,"WantRunning":false}"#
    )?);

    assert_eq!(
        build_tailscale_https_base_url("My-Box.tail.ts.net.", Some(443))?,
        "https://my-box.tail.ts.net/"
    );
    assert_eq!(
        build_tailscale_https_base_url("my-box.tail.ts.net", Some(8443))?,
        "https://my-box.tail.ts.net:8443/"
    );
    assert!(matches!(
        build_tailscale_https_base_url("bad/host", None),
        Err(TailscaleError::InvalidMagicDnsName { .. })
    ));

    assert_eq!(
        classify_stderr_diagnostic("tailscaled.service not running"),
        Some(TailscaleStderrDiagnostic::NotRunning)
    );
    assert_eq!(
        classify_stderr_diagnostic("node token xyz-secret failed"),
        Some(TailscaleStderrDiagnostic::Unknown)
    );
    assert_eq!(
        TailscaleError::TailscaleUnavailable.to_string(),
        "Could not talk to Tailscale. Is tailscaled running? Try `tailscale status`."
    );
    assert_eq!(
        TailscaleError::MagicDnsNameMissing.to_string(),
        "This machine has no MagicDNS name. Run `tailscale up` and enable MagicDNS."
    );
    assert_eq!(
        TailscaleError::ServesOtherEnvironment { serve_port: 8443 }.to_string(),
        "Tailscale Serve on HTTPS port 8443 already fronts a different LegionCode server. Pass --tailscale-serve-port to publish this one on another port."
    );
    assert_eq!(
        TailscaleError::TailscaleServeTargetRefused {
            local_host: "100.64.0.1".to_owned()
        }
        .to_string(),
        "This Tailscale install can only proxy to localhost, and the server listens on 100.64.0.1. Restart it with `--host 127.0.0.1` or `--host 0.0.0.0`, then run this command again."
    );
    let _unused_path_sanity = Path::new("/tmp");
    Ok(())
}

#[test]
fn tailscale_serve_routes_and_root_holder_classification() -> TestResult {
    let target = ServeTarget {
        host: "127.0.0.1".to_owned(),
        port: 3773,
    };
    assert_eq!(
        ensure_tailscale_serve_args(3773, None, None),
        vec!["serve", "--bg", "--https=443", "http://127.0.0.1:3773"]
    );

    let json = r#"{
        "TCP": {"443": {"TCPForward": "unix:/tmp/legioncode.sock"}, "5000": {"TCPForward": "0.0.0.0:3773"}},
        "Web": {"box.ts.net:443": {"Handlers": {"/": {"Proxy": "3773"}}}},
        "Services": {"svc:web": {"Web": {"app.ts.net:443": {"Handlers": {"/app": {"Proxy": "http://[::ffff:127.0.0.1]:3773"}}}}}},
        "Foreground": {"sess1": {"Web": {"fg.ts.net:8443": {"Handlers": {"/": {"Proxy": "http://127.0.0.1:3773"}}}}}}
    }"#;
    let routes = parse_tailscale_serve_routes_to(json, &target).ok_or("expected routes")?;
    assert_eq!(routes.len(), 4);
    let fg = routes
        .iter()
        .find(|r| r.owner.as_deref() == Some("foreground"))
        .ok_or("missing foreground route")?;
    assert_eq!(tailscale_serve_off_args(fg), None);

    let svc = routes
        .iter()
        .find(|r| r.owner.as_deref() == Some("svc:web"))
        .ok_or("missing svc route")?;
    assert_eq!(
        tailscale_serve_off_args(svc),
        Some(vec![
            "serve".to_owned(),
            "--service=svc:web".to_owned(),
            "--https=443".to_owned(),
            "--set-path=/app".to_owned(),
            "off".to_owned(),
        ])
    );

    let holders = parse_tailscale_serve_root_holders(json, 443, &target).ok_or("holders")?;
    assert!(holders.contains(&TailscaleServeRootHolder::Ours));
    assert_eq!(
        classify_serve_root_holder(&holders, |_, _| false),
        TailscaleServeRootStatus::Ours
    );

    let other_json =
        r#"{"Web":{"box.ts.net:443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:9999"}}}}}"#;
    let other_holders =
        parse_tailscale_serve_root_holders(other_json, 443, &target).ok_or("other holders")?;
    assert_eq!(
        classify_serve_root_holder(&other_holders, |_, _| false),
        TailscaleServeRootStatus::Dead
    );
    assert_eq!(
        classify_serve_root_holder(&other_holders, |h, p| h == "127.0.0.1" && p == 9999),
        TailscaleServeRootStatus::Other
    );
    Ok(())
}
