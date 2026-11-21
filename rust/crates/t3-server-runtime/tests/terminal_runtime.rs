//! Integration tests for `TerminalRuntime`: VT snapshots, `BUG-08` resume/evict/close, `BUG-25` palette, and child process I/O.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use t3_protocol::provider_env::TerminalResumeStatus;
use t3_server_runtime::{TerminalOpenRequest, TerminalRuntime};
use t3_terminal_vt::snapshot::decode_t3vt;
use t3_terminal_vt::{Rgba, sanitize_bracketed_paste};
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
    assert!(snap.t3vt_bytes.starts_with(b"T3VT"));
    assert_eq!(snap.cols, 60);
    assert_eq!(snap.rows, 20);
    assert!(snap.viewport_text.contains("hello world"));
    assert!(!snap.viewport_text.contains("\x1b[201~"));
    assert_eq!(sanitize_bracketed_paste("ok\x1b[201~bad"), "okbad");

    let decoded = decode_t3vt(&snap.t3vt_bytes)?;
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
    let resumed_decoded = decode_t3vt(&resumed_snap.t3vt_bytes)?;
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
