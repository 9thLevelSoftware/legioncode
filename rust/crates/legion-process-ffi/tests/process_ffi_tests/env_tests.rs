//! Tests for `legion_process_ffi::env::SanitizedChildEnv` and `NODE_OPTIONS` scrubbing (`BUG-09`).

use std::io::Read;

use legion_process_ffi::env::{SanitizedChildEnv, is_blocked_child_env_key, sanitize_node_options};
use legion_process_ffi::supervisor::{JobProcessSupervisor, SpawnSpec};

#[test]
fn blocks_watch_report_and_node_watch_keys() {
    assert!(is_blocked_child_env_key("WATCH_REPORT_DEPENDENCIES"));
    assert!(is_blocked_child_env_key("watch_report_dependencies"));
    assert!(is_blocked_child_env_key("NODE_WATCH_STATE"));
    assert!(is_blocked_child_env_key("node_watch_channel"));
    assert!(is_blocked_child_env_key("   "));
    assert!(!is_blocked_child_env_key("NODE_OPTIONS"));
    assert!(!is_blocked_child_env_key("PATH"));
}

#[test]
fn strips_node_debugger_flags_while_preserving_other_options() {
    assert_eq!(sanitize_node_options(""), None);
    assert_eq!(sanitize_node_options("   \t  "), None);
    assert_eq!(sanitize_node_options("--inspect"), None);
    assert_eq!(sanitize_node_options("--inspect-brk=9229"), None);
    assert_eq!(sanitize_node_options("--inspect-port=0.0.0.0:9229"), None);
    assert_eq!(sanitize_node_options("--inspect-port 9230"), None);
    assert_eq!(sanitize_node_options("\"--inspect-brk\""), None);

    assert_eq!(
        sanitize_node_options(
            "--max-old-space-size=4096 --inspect --inspect-brk=9229 --require \"C:\\my app\\hook.js\" --inspect-port 9231"
        ),
        Some("--max-old-space-size=4096 --require \"C:\\my app\\hook.js\"".to_owned())
    );
    assert_eq!(
        sanitize_node_options("--inspect-port --max-old-space-size=2048"),
        Some("--max-old-space-size=2048".to_owned())
    );
}

#[test]
fn sanitized_child_env_normalizes_case_on_windows_and_preserves_on_unix() -> Result<(), String> {
    let win_env = SanitizedChildEnv::from_entries(
        [
            ("WATCH_REPORT_DEPENDENCIES", "1"),
            ("NODE_WATCH_IPC", "fd3"),
            ("node_options", "--inspect --title=legioncode"),
            ("NODE_OPTIONS", "--max-old-space-size=1024 --inspect-brk"),
            ("CustomVar", "first"),
            ("CUSTOMVAR", "second"),
        ],
        true,
    );
    assert_eq!(win_env.get("WATCH_REPORT_DEPENDENCIES"), None);
    assert_eq!(win_env.get("NODE_WATCH_IPC"), None);
    assert_eq!(
        win_env.get("NODE_OPTIONS"),
        Some("--max-old-space-size=1024")
    );
    assert_eq!(win_env.get("customvar"), Some("second"));
    assert_eq!(win_env.as_map().len(), 2);

    let unix_env = SanitizedChildEnv::from_entries(
        [
            ("WATCH_REPORT_DEPENDENCIES", "1"),
            ("NODE_WATCH_FD", "4"),
            ("NODE_OPTIONS", "--inspect-port=9229"),
            ("Path", "kept"),
        ],
        false,
    );
    assert_eq!(unix_env.get("NODE_OPTIONS"), None);
    assert_eq!(unix_env.get("WATCH_REPORT_DEPENDENCIES"), None);
    assert_eq!(unix_env.get("NODE_WATCH_FD"), None);
    assert_eq!(unix_env.get("Path"), Some("kept"));

    let json = win_env.to_json_string().map_err(|e| e.to_string())?;
    let reparsed = SanitizedChildEnv::from_json_str(&json, true).map_err(|e| e.to_string())?;
    assert_eq!(reparsed, win_env);
    Ok(())
}

#[test]
fn spawned_child_receives_sanitized_environment() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut env = SanitizedChildEnv::from_os_env();
    env.set("LEGION_PROCESS_FFI_CHILD_ROLE", "print_env");
    env.set("WATCH_REPORT_DEPENDENCIES", "1");
    env.set("NODE_WATCH_TOKEN", "secret");
    env.set(
        "NODE_OPTIONS",
        "--max-old-space-size=512 --inspect --inspect-brk=9229",
    );
    env.set("LEGION_KEEP_MARKER", "present");

    let spec = SpawnSpec::new(exe)
        .args([
            "--exact",
            "process_ffi_tests::supervisor_tests::child_role_entrypoint",
            "--nocapture",
        ])
        .env(env);
    let mut sup = JobProcessSupervisor::new().map_err(|e| e.to_string())?;
    let (token, mut pipes) = sup.spawn(&spec).map_err(|e| e.to_string())?;
    let mut out = String::new();
    if let Some(mut stdout) = pipes.take_stdout() {
        stdout.read_to_string(&mut out).map_err(|e| e.to_string())?;
    }
    let status = sup.wait(token).map_err(|e| e.to_string())?;
    assert!(status.success(), "child status failed; stdout={out}");
    assert!(out.contains("WATCH=NONE;"), "stdout={out}");
    assert!(out.contains("NODE_WATCH=NONE;"), "stdout={out}");
    assert!(
        out.contains("NODE_OPTIONS=--max-old-space-size=512;"),
        "stdout={out}"
    );
    assert!(out.contains("KEEP=present;"), "stdout={out}");
    Ok(())
}
