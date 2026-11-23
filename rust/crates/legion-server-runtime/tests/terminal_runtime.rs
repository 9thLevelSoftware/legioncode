//! Integration tests for `TerminalRuntime`: VT snapshots, `BUG-08` resume/evict/close, `BUG-25` palette, and child process I/O.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use legion_protocol::provider_env::TerminalResumeStatus;
use legion_server_runtime::terminal_session::{
    TerminalMetadataStreamEvent, TerminalStreamChunk, resolve_default_shell_candidates,
};
use legion_server_runtime::{TerminalOpenRequest, TerminalRuntime};
use legion_terminal_vt::snapshot::decode_lcvt;
use legion_terminal_vt::{Rgba, sanitize_bracketed_paste};
use tempfile::TempDir;
use tokio::sync::Notify;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const ECHO_CHILD_SRC: &str = r#"
use std::io::{self, BufRead, Write};
fn main() {
    println!("READY_FROM_CHILD");
    let _ = io::stdout().flush();
    let stdin = io::stdin();
    for line in stdin.lock().lines().map_while(Result::ok) {
        if line == "QUIT" {
            break;
        }
        println!("ECHO:{line}");
        let _ = io::stdout().flush();
    }
}
"#;

fn compile_echo_binary(dir: &Path) -> Result<OsString, Box<dyn std::error::Error>> {
    let src_path = dir.join("mock_term_child.rs");
    let bin_name = format!("mock_term_child{}", std::env::consts::EXE_SUFFIX);
    let bin_path = dir.join(bin_name);
    std::fs::write(&src_path, ECHO_CHILD_SRC)?;
    let out = Command::new("rustc")
        .arg(&src_path)
        .arg("-o")
        .arg(&bin_path)
        .output()?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(format!("rustc failed: {stderr}").into());
    }
    Ok(bin_path.into_os_string())
}

#[test]
fn terminal_vt_palette_resize_snapshot_and_bug08_eviction_vs_close() -> TestResult {
    let runtime = TerminalRuntime::with_capacity(1);

    let open1 = runtime.terminal_open(&TerminalOpenRequest::new("t1", "term-a", 40, 12))?;
    assert!(!open1.resumed);
    assert_eq!(open1.cols, 40);

    runtime.apply_palette_overrides("t1", "term-a", &[(1, Rgba::rgb(12, 34, 56))])?;
    runtime.terminal_write("t1", "term-a", "hello ", false)?;
    runtime.terminal_write("t1", "term-a", "world\x1b[201~spoof", true)?;
    runtime.terminal_resize("t1", "term-a", 60, 20)?;

    let snap = runtime.terminal_snapshot("t1", "term-a")?;
    assert!(snap.lcvt_bytes.starts_with(b"LCVT"));
    assert_eq!(snap.cols, 60);
    assert_eq!(snap.rows, 20);
    assert!(snap.viewport_text.contains("hello world"));
    assert!(!snap.viewport_text.contains("\x1b[201~"));
    assert_eq!(sanitize_bracketed_paste("ok\x1b[201~bad"), "okbad");

    let decoded = decode_lcvt(&snap.lcvt_bytes)?;
    assert_eq!(decoded.cols, 60);
    assert_eq!(decoded.rows, 20);
    assert_eq!(decoded.palette.resolve(1), Rgba::rgb(12, 34, 56));

    // Mark term-a inactive, then open & mark term-b inactive -> capacity=1 evicts term-a!
    let evicted1 = runtime.mark_inactive("t1", "term-a")?;
    assert_eq!(evicted1.len(), 0);

    let _ = runtime.terminal_open(&TerminalOpenRequest::new("t1", "term-b", 80, 24))?;
    let evicted2 = runtime.mark_inactive("t1", "term-b")?;
    assert_eq!(evicted2.len(), 1);
    assert_eq!(evicted2[0].terminal_id, "term-a");
    assert_eq!(
        runtime.resume_status("t1", "term-a"),
        TerminalResumeStatus::EvictedResumable
    );

    // BUG-08: LRU-evicted session resumes cleanly with preserved dimensions & palette.
    let mut resume_req = TerminalOpenRequest::new("t1", "term-a", 10, 5);
    resume_req.resume = true;
    let open_resumed = runtime.terminal_open(&resume_req)?;
    assert!(open_resumed.resumed);
    assert!(open_resumed.was_evicted);
    assert_eq!(open_resumed.cols, 60);
    assert_eq!(open_resumed.rows, 20);
    let resumed_snap = runtime.terminal_snapshot("t1", "term-a")?;
    let resumed_decoded = decode_lcvt(&resumed_snap.lcvt_bytes)?;
    assert_eq!(resumed_decoded.palette.resolve(1), Rgba::rgb(12, 34, 56));

    // BUG-08: Explicit close permanently prevents `resume: true`.
    runtime.terminal_close("t1", "term-b")?;
    assert_eq!(
        runtime.resume_status("t1", "term-b"),
        TerminalResumeStatus::Closed
    );
    let mut resume_b = TerminalOpenRequest::new("t1", "term-b", 80, 24);
    resume_b.resume = true;
    assert!(runtime.terminal_open(&resume_b).is_err());

    let (_, _, evicted_count, closed_count) = runtime.counts();
    assert_eq!(evicted_count, 0);
    assert_eq!(closed_count, 1);
    Ok(())
}

struct CloseTerminalOnDrop<'a> {
    runtime: &'a TerminalRuntime,
    thread_id: &'a str,
    terminal_id: &'a str,
}

impl Drop for CloseTerminalOnDrop<'_> {
    fn drop(&mut self) {
        let _ = self
            .runtime
            .terminal_close(self.thread_id, self.terminal_id);
    }
}

async fn wait_for_viewport_text(
    runtime: &TerminalRuntime,
    notifier: &Notify,
    thread_id: &str,
    terminal_id: &str,
    needle: &str,
) -> TestResult {
    let wait_fut = async {
        loop {
            let output_waiter = notifier.notified();
            tokio::pin!(output_waiter);
            // Arm before inspecting snapshot so `notify_waiters()` cannot fire unnoticed between check and `.await`.
            let _ = output_waiter.as_mut().enable();
            let snap = runtime.terminal_snapshot(thread_id, terminal_id)?;
            if snap.viewport_text.contains(needle) {
                return Ok::<(), Box<dyn std::error::Error>>(());
            }
            output_waiter.await;
        }
    };
    if let Ok(res) = tokio::time::timeout(Duration::from_secs(10), wait_fut).await {
        res
    } else {
        let final_snapshot = runtime
            .terminal_snapshot(thread_id, terminal_id)
            .map_or_else(|e| format!("<snapshot error: {e}>"), |s| s.viewport_text);
        Err(format!(
            "timed out after 10s waiting for `{needle}` in `{thread_id}/{terminal_id}`; final viewport_text:\n{final_snapshot}"
        )
        .into())
    }
}

#[tokio::test]
async fn terminal_runtime_attaches_live_child_process_stdio() -> TestResult {
    let dir = TempDir::new()?;
    let bin = compile_echo_binary(dir.path())?;
    let runtime = TerminalRuntime::default();

    let mut req = TerminalOpenRequest::new("t-proc", "term-live", 80, 24);
    req.command = Some(bin);
    req.cwd = Some(dir.path().to_path_buf());
    req.env_overrides = BTreeMap::new();

    let _ = runtime.terminal_open(&req)?;
    let _cleanup = CloseTerminalOnDrop {
        runtime: &runtime,
        thread_id: "t-proc",
        terminal_id: "term-live",
    };
    let notifier = runtime
        .output_notifier("t-proc", "term-live")
        .ok_or("missing terminal output notifier")?;

    wait_for_viewport_text(
        &runtime,
        &notifier,
        "t-proc",
        "term-live",
        "READY_FROM_CHILD",
    )
    .await?;

    runtime.terminal_write("t-proc", "term-live", "ping123\n", false)?;
    wait_for_viewport_text(&runtime, &notifier, "t-proc", "term-live", "ECHO:ping123").await?;

    runtime.terminal_close("t-proc", "term-live")?;
    assert_eq!(
        runtime.resume_status("t-proc", "term-live"),
        TerminalResumeStatus::Closed
    );
    Ok(())
}

#[test]
fn default_shell_candidates_include_os_fallback() {
    let mut overrides = BTreeMap::new();
    let candidates = resolve_default_shell_candidates(&overrides);
    assert_ne!(candidates, Vec::<OsString>::new());

    #[cfg(windows)]
    {
        overrides.insert(
            "COMSPEC".to_owned(),
            "C:\\Windows\\System32\\cmd.exe".to_owned(),
        );
        let with_comspec = resolve_default_shell_candidates(&overrides);
        assert!(
            with_comspec
                .iter()
                .any(|s| s.to_string_lossy().to_ascii_lowercase().contains("cmd.exe"))
        );
    }
    #[cfg(not(windows))]
    {
        overrides.insert("SHELL".to_owned(), "/bin/sh".to_owned());
        let with_shell = resolve_default_shell_candidates(&overrides);
        assert_eq!(
            with_shell.first().map(|s| s.to_string_lossy()),
            Some("/bin/sh".into())
        );
    }
}

async fn await_stream_contains(
    runtime: &TerminalRuntime,
    rx: &mut tokio::sync::broadcast::Receiver<TerminalStreamChunk>,
    initial_history: &str,
    initial_seq: u64,
    needle: &str,
) -> TestResult {
    let mut combined = initial_history.to_owned();
    tokio::time::timeout(Duration::from_secs(10), async {
        while !combined.contains(needle) {
            match rx.recv().await {
                Ok(TerminalStreamChunk::Output { data, sequence, .. }) => {
                    if sequence > initial_seq {
                        combined.push_str(&data);
                    }
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    combined = runtime.session_snapshot("t-pty", "term-pty", None)?.history;
                }
                Err(err) => return Err(format!("stream closed early: {err}").into()),
            }
        }
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .await
    .map_err(|_| format!("timed out waiting for `{needle}`"))?
}

async fn await_resize_clear_exit(
    runtime: &TerminalRuntime,
    rx: &mut tokio::sync::broadcast::Receiver<TerminalStreamChunk>,
) -> TestResult {
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut saw_cleared = false;
        let mut saw_after_clear = false;
        let mut saw_resize = false;
        loop {
            match rx.recv().await {
                Ok(TerminalStreamChunk::Activity {
                    cols: Some(110),
                    rows: Some(35),
                    ..
                }) => saw_resize = true,
                Ok(TerminalStreamChunk::Cleared { .. }) => saw_cleared = true,
                Ok(TerminalStreamChunk::Output { data, .. }) if data.contains("AFTER_CLEAR") => {
                    saw_after_clear = true;
                }
                Ok(TerminalStreamChunk::Exited { exit_code, .. }) => {
                    assert_eq!(exit_code, Some(0));
                    break;
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    let s = runtime.session_snapshot("t-pty", "term-pty", None)?;
                    saw_after_clear |= s.history.contains("AFTER_CLEAR");
                    if s.exit_code == Some(0) {
                        break;
                    }
                }
                Err(err) => return Err(format!("stream closed before exit: {err}").into()),
            }
        }
        assert!(saw_resize, "expected Activity resize event");
        assert!(saw_cleared, "expected Cleared event");
        assert!(
            saw_after_clear
                || runtime
                    .session_snapshot("t-pty", "term-pty", None)?
                    .history
                    .contains("AFTER_CLEAR"),
            "expected AFTER_CLEAR output"
        );
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .await
    .map_err(|_| "timed out waiting for PTY resize/clear/exit")?
}

#[tokio::test]
async fn terminal_runtime_interactive_pty_streams_resize_clear_exit_and_metadata() -> TestResult {
    let runtime = TerminalRuntime::default();
    let (meta_snapshot, mut meta_rx) = runtime.subscribe_metadata()?;
    assert!(matches!(
        meta_snapshot,
        TerminalMetadataStreamEvent::Snapshot { terminals } if terminals.is_empty()
    ));

    let mut req = TerminalOpenRequest::interactive("t-pty", "term-pty", 80, 24);
    req.env_overrides
        .insert("LEGIONCODE_CUSTOM".to_owned(), "ok".to_owned());
    let open = runtime.terminal_open(&req)?;
    let _cleanup = CloseTerminalOnDrop {
        runtime: &runtime,
        thread_id: "t-pty",
        terminal_id: "term-pty",
    };
    assert_eq!(open.cols, 80);

    let sub = runtime.subscribe_output("t-pty", "term-pty")?;
    assert!(sub.snapshot.pid.is_some());
    assert_eq!(sub.snapshot.cols, 80);
    assert_eq!(sub.snapshot.rows, 24);
    let mut rx = sub.receiver;

    #[cfg(windows)]
    let cmd_echo = "echo RUNTIME_PTY_STREAM_OK\r\n";
    #[cfg(not(windows))]
    let cmd_echo = "echo RUNTIME_PTY_STREAM_OK\n";
    runtime.terminal_write("t-pty", "term-pty", cmd_echo, false)?;
    await_stream_contains(
        &runtime,
        &mut rx,
        &sub.snapshot.history,
        sub.snapshot.sequence,
        "RUNTIME_PTY_STREAM_OK",
    )
    .await?;

    runtime.terminal_resize("t-pty", "term-pty", 110, 35)?;
    let resized_snap = runtime.terminal_snapshot("t-pty", "term-pty")?;
    assert_eq!(resized_snap.cols, 110);
    assert_eq!(resized_snap.rows, 35);

    runtime.terminal_clear("t-pty", "term-pty")?;
    let cleared_state = runtime.session_snapshot("t-pty", "term-pty", None)?;
    assert_eq!(cleared_state.history, "");

    #[cfg(windows)]
    let cmd_exit = "echo AFTER_CLEAR\r\nexit 0\r\n";
    #[cfg(not(windows))]
    let cmd_exit = "echo AFTER_CLEAR\nexit 0\n";
    runtime.terminal_write("t-pty", "term-pty", cmd_exit, false)?;
    await_resize_clear_exit(&runtime, &mut rx).await?;

    let late_sub = runtime.subscribe_output("t-pty", "term-pty")?;
    assert!(late_sub.snapshot.history.contains("AFTER_CLEAR"));

    runtime.terminal_close("t-pty", "term-pty")?;
    let mut saw_remove = false;
    while let Ok(ev) = meta_rx.try_recv() {
        if matches!(
            ev,
            TerminalMetadataStreamEvent::Remove { ref thread_id, ref terminal_id }
                if thread_id == "t-pty" && terminal_id == "term-pty"
        ) {
            saw_remove = true;
        }
    }
    assert!(saw_remove, "expected metadata Remove on terminal_close");
    Ok(())
}
