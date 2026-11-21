//! Integration tests covering all six `__ssh-helper` operations (`ssh_helper`).

use std::fs;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::SystemTime;

use rusqlite::Connection;
use serde_json::json;
use t3_persistence::ownership::format_iso8601_utc;
use t3_remote::ssh_helper::{
    DESKTOP_UPDATE_IN_PROGRESS_TTL_MS, format_picked_port, format_remaining_update_wait,
    format_runtime_port, is_home_owned, keep_server, keeps_running_server, pick_port,
    remaining_update_wait_ms, runtime_port, wait_ready,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn ssh_helper_keep_server_matches_ts_test_vectors() -> TestResult {
    let runtime = |extra: serde_json::Value| -> String {
        let mut obj = json!({ "version": 1, "pid": 42, "port": 3773 });
        if let (Some(dst), Some(src)) = (obj.as_object_mut(), extra.as_object()) {
            for (k, v) in src {
                dst.insert(k.clone(), v.clone());
            }
        }
        obj.to_string()
    };

    assert!(keeps_running_server(
        &runtime(json!({ "serverVersion": "0.1.6" })),
        42,
        "0.1.6"
    ));
    assert!(keeps_running_server(
        &runtime(json!({ "serverVersion": "0.1.7-nightly.20261102.4080" })),
        42,
        "0.1.6"
    ));
    assert!(!keeps_running_server(
        &runtime(json!({ "serverVersion": "0.1.7-nightly.20261102.4080" })),
        42,
        "0.1.7"
    ));
    assert!(!keeps_running_server(
        &runtime(json!({ "serverVersion": "0.1.5" })),
        42,
        "0.1.6"
    ));
    assert!(!keeps_running_server(
        &runtime(json!({ "serverVersion": "0.1.6" })),
        7,
        "0.1.6"
    ));
    assert!(!keeps_running_server(&runtime(json!({})), 42, "0.1.6"));
    assert!(!keeps_running_server(
        &runtime(json!({ "serverVersion": "latest" })),
        42,
        "0.1.6"
    ));
    assert!(!keeps_running_server("", 42, "0.1.6"));

    let dir = tempfile::tempdir()?;
    let rt_path = dir.path().join("server-runtime.json");
    assert!(!keep_server(&rt_path, 42, "0.1.6"));
    fs::write(&rt_path, runtime(json!({ "serverVersion": "0.1.8" })))?;
    assert!(keep_server(&rt_path, 42, "0.1.6"));
    Ok(())
}

#[test]
fn ssh_helper_runtime_port_and_exact_stdout_formatting() -> TestResult {
    let dir = tempfile::tempdir()?;
    let rt_path = dir.path().join("server-runtime.json");
    let pid = std::process::id();

    fs::write(
        &rt_path,
        json!({
            "version": 1,
            "pid": pid,
            "port": 4321,
            "origin": "http://localhost:4321/"
        })
        .to_string(),
    )?;
    assert_eq!(runtime_port(&rt_path), Some((pid, 4321)));
    let formatted = format_runtime_port(pid, 4321);
    assert_eq!(formatted, format!("{pid} 4321"));
    assert!(!formatted.ends_with('\n'));

    // Reject non-loopback origin or HTTPS or dead PID.
    fs::write(
        &rt_path,
        json!({ "pid": pid, "port": 4321, "origin": "https://127.0.0.1:4321" }).to_string(),
    )?;
    assert_eq!(runtime_port(&rt_path), None);

    fs::write(
        &rt_path,
        json!({ "pid": 999_999_999, "port": 4321, "origin": "http://127.0.0.1:4321" }).to_string(),
    )?;
    assert_eq!(runtime_port(&rt_path), None);
    Ok(())
}

#[test]
fn ssh_helper_pick_port_and_wait_ready() -> TestResult {
    let dir = tempfile::tempdir()?;
    let port_file = dir.path().join("port");
    let occupied = (28_700..28_900)
        .find_map(|p| TcpListener::bind((Ipv4Addr::LOCALHOST, p)).ok())
        .ok_or("expected bindable loopback port")?;
    let busy_port = occupied.local_addr()?.port();
    fs::write(&port_file, format!("{busy_port}\n"))?;

    let picked = pick_port(&port_file, 3773, 200).ok_or("expected free loopback port")?;
    assert_ne!(picked, busy_port);
    assert!(picked > busy_port && picked < busy_port + 200);
    assert!(!format_picked_port(picked).ends_with('\n'));
    drop(occupied);

    // wait_ready retries past initial 503 until 200 OK.
    let probe_listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let probe_port = probe_listener.local_addr()?.port();
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_srv = Arc::clone(&hits);
    let handle = std::thread::spawn(move || {
        for stream in probe_listener.incoming().flatten().take(2) {
            let mut s = stream;
            let mut buf = [0u8; 512];
            let _ = s.read(&mut buf);
            let n = hits_srv.fetch_add(1, Ordering::SeqCst);
            let resp = if n == 0 {
                "HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            } else {
                "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok"
            };
            let _ = s.write_all(resp.as_bytes());
        }
    });

    assert!(wait_ready(probe_port, 3_000, 1_000));
    assert!(hits.load(Ordering::SeqCst) >= 2);
    let _ = handle.join();
    Ok(())
}

#[test]
fn ssh_helper_remaining_update_wait_and_home_owned() -> TestResult {
    assert_eq!(DESKTOP_UPDATE_IN_PROGRESS_TTL_MS, 60_000);
    let dir = tempfile::tempdir()?;
    let marker = dir.path().join("update-in-progress.json");
    assert_eq!(remaining_update_wait_ms(&marker, 10_000), None);
    assert_eq!(format_remaining_update_wait(None), "");

    // Capped at TTL (60_000ms).
    fs::write(&marker, r#"{"startedAt":1000,"expiresAt":200000}"#)?;
    assert_eq!(remaining_update_wait_ms(&marker, 5_000), Some(60_000));
    assert_eq!(format_remaining_update_wait(Some(60_000)), "60000");

    // Expired or clock-skewed marker returns None.
    assert_eq!(remaining_update_wait_ms(&marker, 500), None);
    assert_eq!(remaining_update_wait_ms(&marker, 200_000), None);

    // Home ownership via service-serving.lock OR live legacy runtime PID.
    let home = dir.path().join("home");
    fs::create_dir_all(home.join("runtime"))?;
    fs::create_dir_all(home.join("userdata"))?;
    assert!(!is_home_owned(&home));

    let serving_lock = home.join("runtime").join("service-serving.lock");
    let conn = Connection::open(&serving_lock)?;
    conn.execute_batch("BEGIN EXCLUSIVE;")?;
    assert!(is_home_owned(&home));
    drop(conn);
    assert!(!is_home_owned(&home));

    // Pre-lock legacy server-runtime.json with current process PID (startedAt >= process start) is owned.
    fs::write(
        home.join("userdata").join("server-runtime.json"),
        json!({
            "version": 1,
            "pid": std::process::id(),
            "port": 3773,
            "origin": "http://127.0.0.1:3773",
            "startedAt": format_iso8601_utc(SystemTime::now())
        })
        .to_string(),
    )?;
    assert!(is_home_owned(&home));
    Ok(())
}
