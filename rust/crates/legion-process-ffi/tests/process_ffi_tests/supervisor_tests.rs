//! Integration tests for `ScopedPipeSet`, `JobHandle`, and `JobProcessSupervisor` (`BUG-09`).

use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::time::Duration;

use legion_process_ffi::env::SanitizedChildEnv;
use legion_process_ffi::pipes::{PipeSetConfig, ScopedPipeSet, StdioMode};
use legion_process_ffi::supervisor::{JobProcessSupervisor, SpawnSpec, SupervisorError};
use legion_process_ffi::win32_job::{JobHandle, Win32JobError};

#[test]
fn child_role_entrypoint() -> Result<(), String> {
    let Ok(role) = std::env::var("LEGION_PROCESS_FFI_CHILD_ROLE") else {
        return Ok(());
    };
    match role.as_str() {
        "print_env" => {
            let watch = std::env::var("WATCH_REPORT_DEPENDENCIES").unwrap_or("NONE".to_owned());
            let nw = std::env::var("NODE_WATCH_TOKEN").unwrap_or("NONE".to_owned());
            let opts = std::env::var("NODE_OPTIONS").unwrap_or("NONE".to_owned());
            let keep = std::env::var("LEGION_KEEP_MARKER").unwrap_or("NONE".to_owned());
            let line = format!("WATCH={watch};NODE_WATCH={nw};NODE_OPTIONS={opts};KEEP={keep};\n");
            let mut out = std::io::stdout().lock();
            out.write_all(line.as_bytes()).map_err(|e| e.to_string())?;
            out.flush().map_err(|e| e.to_string())?;
        }
        "tree_parent" => {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let addr = std::env::var("LEGION_TEST_ADDR").map_err(|e| e.to_string())?;
            let mut grandchild = Command::new(exe)
                .args([
                    "--exact",
                    "process_ffi_tests::supervisor_tests::child_role_entrypoint",
                ])
                .env("LEGION_PROCESS_FFI_CHILD_ROLE", "tree_child")
                .env("LEGION_TEST_ADDR", addr)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|e| e.to_string())?;
            let _ = grandchild.wait();
        }
        "tree_child" => {
            let addr = std::env::var("LEGION_TEST_ADDR").map_err(|e| e.to_string())?;
            let mut stream = TcpStream::connect(addr).map_err(|e| e.to_string())?;
            writeln!(stream, "{}", std::process::id()).map_err(|e| e.to_string())?;
            stream.flush().map_err(|e| e.to_string())?;
            let mut hold = [0u8; 8];
            let _ = stream.read(&mut hold);
        }
        "early_pipe_drop" => verify_scoped_pipe_set_early_drop()?,
        _ => {}
    }
    Ok(())
}

fn verify_scoped_pipe_set_early_drop() -> Result<(), String> {
    let pipes = ScopedPipeSet::new(PipeSetConfig {
        stdin: StdioMode::Piped,
        stdout: StdioMode::Piped,
        stderr: StdioMode::Piped,
        extra_outputs: 1,
    })
    .map_err(|e| e.to_string())?;
    assert_eq!(pipes.child_open_count(), 4);

    let (child_guard, mut parent) = pipes.split();
    assert_eq!(child_guard.open_child_count(), 4);
    assert!(!child_guard.is_committed());

    // Drop child-side RAII guard before spawn to simulate early pre-spawn validation error.
    drop(child_guard);

    let mut buf = [0u8; 16];
    let mut stdout = parent.take_stdout().ok_or("missing parent stdout")?;
    assert_eq!(stdout.read(&mut buf).map_err(|e| e.to_string())?, 0);

    let mut stderr = parent.take_stderr().ok_or("missing parent stderr")?;
    assert_eq!(stderr.read(&mut buf).map_err(|e| e.to_string())?, 0);

    let mut extra = parent.extra_outputs.pop().ok_or("missing extra output")?;
    assert_eq!(extra.read(&mut buf).map_err(|e| e.to_string())?, 0);

    let mut stdin = parent.take_stdin().ok_or("missing parent stdin")?;
    let write_err = stdin
        .write_all(b"ping")
        .err()
        .ok_or("expected BrokenPipe on closed stdin")?;
    assert_eq!(write_err.kind(), ErrorKind::BrokenPipe);
    Ok(())
}

#[test]
fn scoped_pipe_set_closes_child_descriptors_on_early_drop_before_spawn() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let output = Command::new(exe)
        .args([
            "--exact",
            "process_ffi_tests::supervisor_tests::child_role_entrypoint",
            "--test-threads=1",
        ])
        .env("LEGION_PROCESS_FFI_CHILD_ROLE", "early_pipe_drop")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "isolated early_pipe_drop child failed ({:?}):\nstdout: {}\nstderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

#[test]
fn supervisor_reports_pre_spawn_and_missing_binary_errors_cleanly() -> Result<(), String> {
    let mut sup = JobProcessSupervisor::new().map_err(|e| e.to_string())?;
    let empty_err = sup
        .spawn(&SpawnSpec::new(""))
        .err()
        .ok_or("expected EmptyProgram error")?;
    assert!(matches!(empty_err, SupervisorError::EmptyProgram));

    let missing_err = sup
        .spawn(&SpawnSpec::new("legion-nonexistent-binary-99887766"))
        .err()
        .ok_or("expected SpawnFailed error")?;
    assert!(matches!(missing_err, SupervisorError::SpawnFailed(_)));
    Ok(())
}

#[test]
fn supervisor_terminate_tree_kills_real_process_tree() -> Result<(), String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let addr = listener
        .local_addr()
        .map_err(|e| e.to_string())?
        .to_string();
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;

    let mut env = SanitizedChildEnv::from_os_env();
    env.set("LEGION_PROCESS_FFI_CHILD_ROLE", "tree_parent");
    env.set("LEGION_TEST_ADDR", addr);

    let spec = SpawnSpec::new(exe)
        .args([
            "--exact",
            "process_ffi_tests::supervisor_tests::child_role_entrypoint",
        ])
        .env(env)
        .stdio(PipeSetConfig::null());

    let mut sup = JobProcessSupervisor::new().map_err(|e| e.to_string())?;
    let (token, _pipes) = sup.spawn(&spec).map_err(|e| e.to_string())?;
    let parent_pid = sup.pid(token).ok_or("missing spawned parent pid")?;
    assert!(parent_pid > 0);

    let (mut socket, _) = listener.accept().map_err(|e| e.to_string())?;
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(socket.try_clone().map_err(|e| e.to_string())?);
    let mut pid_line = String::new();
    reader.read_line(&mut pid_line).map_err(|e| e.to_string())?;
    let grandchild_pid: u32 = pid_line.trim().parse().map_err(|e| format!("{e}"))?;
    assert!(grandchild_pid > 0);

    sup.terminate_tree(42).map_err(|e| e.to_string())?;

    // Wait for kernel-closed grandchild socket (or Windows job exit).
    let mut eof_buf = [0u8; 8];
    let closed = matches!(socket.read(&mut eof_buf), Ok(0) | Err(_));
    if cfg!(windows) {
        assert!(
            closed,
            "grandchild socket remained open after kernel Job terminate"
        );
    }
    Ok(())
}

#[test]
fn win32_job_handle_drop_kills_process_tree_or_reports_unsupported() -> Result<(), String> {
    if !cfg!(windows) {
        assert!(!legion_process_ffi::win32_job::is_supported());
        let err = JobHandle::new_kill_on_close()
            .err()
            .ok_or("expected UnsupportedPlatform")?;
        assert!(matches!(err, Win32JobError::UnsupportedPlatform));
        return Ok(());
    }

    assert!(legion_process_ffi::win32_job::is_supported());
    if cfg!(target_pointer_width = "64") {
        assert_eq!(JobHandle::extended_limit_info_size(), 144);
    }

    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let addr = listener
        .local_addr()
        .map_err(|e| e.to_string())?
        .to_string();
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;

    let job = JobHandle::new_kill_on_close().map_err(|e| e.to_string())?;
    let mut parent = Command::new(exe)
        .args([
            "--exact",
            "process_ffi_tests::supervisor_tests::child_role_entrypoint",
        ])
        .env("LEGION_PROCESS_FFI_CHILD_ROLE", "tree_parent")
        .env("LEGION_TEST_ADDR", addr)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    job.assign_child(&parent).map_err(|e| e.to_string())?;

    let (mut socket, _) = listener.accept().map_err(|e| e.to_string())?;
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(socket.try_clone().map_err(|e| e.to_string())?);
    let mut pid_line = String::new();
    reader.read_line(&mut pid_line).map_err(|e| e.to_string())?;
    assert_ne!(pid_line.trim(), "");

    // Dropping `JobHandle` closes the handle and triggers `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`.
    drop(job);

    let mut eof_buf = [0u8; 8];
    let closed = matches!(socket.read(&mut eof_buf), Ok(0) | Err(_));
    assert!(closed, "grandchild survived JobHandle drop");
    let _ = parent.wait();
    Ok(())
}

#[test]
fn job_handle_assign_pid_terminates_detached_process() -> Result<(), String> {
    if !cfg!(windows) {
        return Ok(());
    }
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let addr = listener
        .local_addr()
        .map_err(|e| e.to_string())?
        .to_string();
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut child = Command::new(exe)
        .args([
            "--exact",
            "process_ffi_tests::supervisor_tests::child_role_entrypoint",
        ])
        .env("LEGION_PROCESS_FFI_CHILD_ROLE", "tree_child")
        .env("LEGION_TEST_ADDR", addr)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;

    let (mut socket, _) = listener.accept().map_err(|e| e.to_string())?;
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(socket.try_clone().map_err(|e| e.to_string())?);
    let mut pid_line = String::new();
    reader.read_line(&mut pid_line).map_err(|e| e.to_string())?;
    let child_pid: u32 = pid_line.trim().parse().map_err(|e| format!("{e}"))?;

    let job = JobHandle::new_kill_on_close().map_err(|e| e.to_string())?;
    job.assign_pid(child_pid).map_err(|e| e.to_string())?;
    job.terminate(9).map_err(|e| e.to_string())?;

    let mut eof_buf = [0u8; 8];
    let closed = matches!(socket.read(&mut eof_buf), Ok(0) | Err(_));
    assert!(closed, "process assigned by PID survived terminate");
    let _ = child.wait();
    Ok(())
}

#[test]
fn wait_timeout_reports_none_while_running_and_some_when_finished() -> Result<(), String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let addr = listener
        .local_addr()
        .map_err(|e| e.to_string())?
        .to_string();
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut env = SanitizedChildEnv::from_os_env();
    env.set("LEGION_PROCESS_FFI_CHILD_ROLE", "tree_child");
    env.set("LEGION_TEST_ADDR", &addr);
    let spec = SpawnSpec::new(exe)
        .args([
            "--exact",
            "process_ffi_tests::supervisor_tests::child_role_entrypoint",
        ])
        .env(env)
        .stdio(PipeSetConfig::null());

    let mut sup = JobProcessSupervisor::new().map_err(|e| e.to_string())?;
    let (token, _pipes) = sup.spawn(&spec).map_err(|e| e.to_string())?;
    let (socket, _) = listener.accept().map_err(|e| e.to_string())?;

    assert!(
        sup.wait_timeout(token, Duration::ZERO)
            .map_err(|e| e.to_string())?
            .is_none()
    );
    drop(socket);
    let exited = sup
        .wait_timeout(token, Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    assert!(exited.is_some(), "child should exit once socket closes");
    Ok(())
}
