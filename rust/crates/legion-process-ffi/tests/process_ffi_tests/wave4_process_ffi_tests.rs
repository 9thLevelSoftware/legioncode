//! Tests for `spawn_flags` (#38194), `keep_awake` (#39490), and `async_log` (#39857).

use std::io::{self, Write};
use std::sync::{Arc, Condvar, Mutex};

use legion_process_ffi::async_log::{AsyncLogConfig, AsyncLogWriter, LogEnqueueStatus};
use legion_process_ffi::keep_awake::{
    FORBIDDEN_SYSTEMD_INHIBIT_WHAT_IDLE_SLEEP, InhibitorStdin, KeepAwakeError, KeepAwakePlatform,
    SYSTEMD_INHIBIT_WHAT_SLEEP, build_default_keep_awake_spec, build_keep_awake_spec,
    is_wsl_kernel_release,
};
use legion_process_ffi::spawn_flags::{
    CREATE_NO_WINDOW, ChildProcessRole, TargetOs, creation_flags_for, policy_for,
    should_detach_dev_server,
};
use legion_process_ffi::supervisor::SpawnSpec;

#[test]
fn windows_background_roles_apply_create_no_window_and_never_detach_dev_server() {
    for role in [
        ChildProcessRole::DevServer,
        ChildProcessRole::Worker,
        ChildProcessRole::GitSubprocess,
        ChildProcessRole::ProbeSubprocess,
    ] {
        let win = policy_for(role, TargetOs::Windows);
        assert_eq!(win.creation_flags, CREATE_NO_WINDOW);
        assert_eq!(CREATE_NO_WINDOW, 0x0800_0000);
        assert!(win.windows_hide);
        assert!(!win.detached);
        assert!(win.teardown_via_tree_kill);
    }

    let interactive = policy_for(ChildProcessRole::InteractiveConsole, TargetOs::Windows);
    assert_eq!(interactive.creation_flags, 0);
    assert!(!interactive.windows_hide);

    assert!(!should_detach_dev_server(TargetOs::Windows));
    assert!(should_detach_dev_server(TargetOs::Linux));
    assert!(should_detach_dev_server(TargetOs::MacOs));
    assert_eq!(
        creation_flags_for(ChildProcessRole::Worker, TargetOs::Linux),
        0
    );

    let spec = SpawnSpec::for_role("git", ChildProcessRole::GitSubprocess);
    assert!(spec.windows_no_window);
    let interactive_spec = SpawnSpec::for_role("cmd", ChildProcessRole::InteractiveConsole);
    assert!(!interactive_spec.windows_no_window);
}

#[test]
fn linux_keep_awake_uses_sleep_only_and_never_blocks_idle_screen_lock() -> Result<(), String> {
    let spec = build_default_keep_awake_spec(KeepAwakePlatform::Linux, 4242)
        .map_err(|e| e.to_string())?
        .ok_or("expected Linux inhibitor spec")?;

    assert_eq!(spec.program, "systemd-inhibit");
    assert_eq!(spec.stdin, InhibitorStdin::Ignore);
    assert!(spec.uses_linux_sleep_only_inhibit());
    assert!(spec.args.iter().any(|a| a == SYSTEMD_INHIBIT_WHAT_SLEEP));
    assert!(
        !spec
            .args
            .iter()
            .any(|a| a == FORBIDDEN_SYSTEMD_INHIBIT_WHAT_IDLE_SLEEP)
    );
    assert!(spec.args.contains(&"--mode=block".to_owned()));
    assert_eq!(spec.args.last().map(String::as_str), Some("4242"));

    let wsl = build_default_keep_awake_spec(KeepAwakePlatform::LinuxWsl, 99)
        .map_err(|e| e.to_string())?
        .ok_or("expected WSL inhibitor spec")?;
    assert_eq!(wsl.program, "powershell.exe");
    assert_eq!(wsl.stdin, InhibitorStdin::Pipe);
    assert!(!wsl.uses_linux_sleep_only_inhibit());

    let mac = build_default_keep_awake_spec(KeepAwakePlatform::MacOs, 99)
        .map_err(|e| e.to_string())?
        .ok_or("expected macOS inhibitor spec")?;
    assert_eq!(mac.program, "sh");
    assert!(mac.args.iter().any(|a| a.contains("caffeinate -i -w")));

    assert_eq!(
        build_default_keep_awake_spec(KeepAwakePlatform::Other, 99).map_err(|e| e.to_string())?,
        None
    );
    assert!(matches!(
        build_default_keep_awake_spec(KeepAwakePlatform::Linux, 0),
        Err(KeepAwakeError::InvalidServerPid)
    ));
    assert!(matches!(
        build_keep_awake_spec(KeepAwakePlatform::Linux, 1, "bad\nwho", "why"),
        Err(KeepAwakeError::InvalidMetadata { field: "who" })
    ));
    assert!(is_wsl_kernel_release("5.15.146.1-microsoft-standard-WSL2"));
    assert!(!is_wsl_kernel_release("6.8.0-generic"));
    Ok(())
}

struct GateState {
    release: bool,
    entered_write: bool,
    written: Vec<u8>,
}

struct GatedSink {
    gate: Arc<(Mutex<GateState>, Condvar)>,
}

impl Write for GatedSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let (lock, cv) = &*self.gate;
        let mut guard = lock
            .lock()
            .map_err(|_| io::Error::other("poisoned test gate"))?;
        guard.entered_write = true;
        cv.notify_all();
        while !guard.release {
            guard = cv
                .wait(guard)
                .map_err(|_| io::Error::other("poisoned test gate"))?;
        }
        guard.written.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn async_log_writer_never_stalls_on_slow_disk_and_counts_dropped_lines() -> Result<(), String> {
    let gate = Arc::new((
        Mutex::new(GateState {
            release: false,
            entered_write: false,
            written: Vec::new(),
        }),
        Condvar::new(),
    ));
    let writer = AsyncLogWriter::spawn(
        GatedSink {
            gate: Arc::clone(&gate),
        },
        AsyncLogConfig {
            channel_capacity: 2,
            max_pending_bytes: 32,
        },
    )
    .map_err(|e| e.to_string())?;

    // First line starts the write and blocks inside `GatedSink::write`.
    assert_eq!(
        writer.try_write_line("first-line"),
        LogEnqueueStatus::Queued
    );
    {
        let (lock, cv) = &*gate;
        let mut guard = lock.lock().map_err(|e| e.to_string())?;
        while !guard.entered_write {
            guard = cv.wait(guard).map_err(|e| e.to_string())?;
        }
    }

    // Fill bounded channel / byte budget, then verify subsequent lines drop immediately without blocking.
    let mut dropped = 0_u64;
    for idx in 0..10 {
        let status = writer.try_write_line(&format!("line-{idx:02}"));
        if matches!(
            status,
            LogEnqueueStatus::DroppedBackpressure | LogEnqueueStatus::DroppedFull
        ) {
            dropped += 1;
        }
    }
    assert!(dropped > 0);
    assert_eq!(writer.dropped_lines(), dropped);

    // Unblock slow sink and drain cleanly.
    {
        let (lock, cv) = &*gate;
        let mut guard = lock.lock().map_err(|e| e.to_string())?;
        guard.release = true;
        cv.notify_all();
    }
    writer.flush().map_err(|e| e.to_string())?;
    let stats = writer.stats();
    assert_eq!(stats.dropped_lines, dropped);
    assert!(stats.written_lines >= 1);
    assert_eq!(stats.pending_bytes, 0);
    writer.shutdown();

    let text = {
        let (lock, _) = &*gate;
        let guard = lock.lock().map_err(|e| e.to_string())?;
        String::from_utf8_lossy(&guard.written).into_owned()
    };
    assert!(text.starts_with("first-line\n"));
    Ok(())
}
