use std::io::Read;
use std::thread;

use legion_process_ffi::pty::{PtyError, PtyProcess, PtySpawnConfig, exit_parts};

fn one_shot_shell_config(cols: u16, rows: u16, command_text: &str) -> PtySpawnConfig {
    #[cfg(windows)]
    {
        PtySpawnConfig::new("cmd.exe", cols, rows)
            .arg("/C")
            .arg(command_text)
    }
    #[cfg(not(windows))]
    {
        PtySpawnConfig::new("/bin/sh", cols, rows)
            .arg("-c")
            .arg(command_text)
    }
}

fn interactive_shell_config(cols: u16, rows: u16) -> PtySpawnConfig {
    #[cfg(windows)]
    {
        PtySpawnConfig::new("cmd.exe", cols, rows).arg("/Q")
    }
    #[cfg(not(windows))]
    {
        PtySpawnConfig::new("/bin/sh", cols, rows)
    }
}

#[test]
fn pty_rejects_empty_program_and_zero_dimensions() {
    let empty_err = PtyProcess::spawn(&PtySpawnConfig::new("", 80, 24)).expect_err("empty program");
    assert!(matches!(empty_err, PtyError::EmptyProgram));

    let dim_err = PtyProcess::spawn(&PtySpawnConfig::new("cmd.exe", 0, 24)).expect_err("zero cols");
    assert!(matches!(
        dim_err,
        PtyError::InvalidDimensions { cols: 0, rows: 24 }
    ));

    let dim_err_rows =
        PtyProcess::spawn(&PtySpawnConfig::new("cmd.exe", 80, 0)).expect_err("zero rows");
    assert!(matches!(
        dim_err_rows,
        PtyError::InvalidDimensions { cols: 80, rows: 0 }
    ));
}

#[test]
fn pty_spawns_shell_reads_output_and_reports_exit_code() {
    assert!(PtyProcess::is_supported());
    let cfg = one_shot_shell_config(80, 24, "echo PTY_OK");
    let pty = PtyProcess::spawn(&cfg).expect("spawn one-shot PTY shell");
    assert!(pty.pid() > 0);

    let mut reader = pty.reader_clone().expect("clone PTY reader");
    let reader_handle = thread::spawn(move || {
        let mut buf = Vec::new();
        reader
            .read_to_end(&mut buf)
            .expect("read PTY stream to EOF");
        String::from_utf8_lossy(&buf).into_owned()
    });

    let status = pty.wait().expect("wait for one-shot PTY child");
    assert!(status.success(), "expected exit 0, got {status:?}");
    assert_eq!(exit_parts(status), (Some(0), None));
    assert!(pty.try_wait().expect("cached try_wait").is_some());

    let output = reader_handle.join().expect("join PTY reader thread");
    assert!(
        output.contains("PTY_OK"),
        "expected PTY_OK in output, got: {output:?}"
    );
}

#[test]
fn pty_interactive_input_resize_and_clean_exit() {
    let cfg = interactive_shell_config(80, 24);
    let pty = PtyProcess::spawn(&cfg).expect("spawn interactive PTY shell");

    let invalid_resize = pty.resize(0, 24);
    assert!(matches!(
        invalid_resize,
        Err(PtyError::InvalidDimensions { cols: 0, rows: 24 })
    ));

    pty.resize(120, 40).expect("resize live PTY");

    let mut reader = pty.reader_clone().expect("clone interactive reader");
    let reader_handle = thread::spawn(move || {
        let mut buf = Vec::new();
        reader
            .read_to_end(&mut buf)
            .expect("read interactive stream");
        String::from_utf8_lossy(&buf).into_owned()
    });

    #[cfg(windows)]
    let script = b"echo PTY_INTERACTIVE_OK\r\nexit 0\r\n";
    #[cfg(not(windows))]
    let script = b"echo PTY_INTERACTIVE_OK\nstty size\nexit 0\n";

    pty.write_all(script).expect("write interactive commands");
    let status = pty.wait().expect("wait interactive exit");
    assert!(status.success(), "expected exit 0, got {status:?}");

    let output = reader_handle.join().expect("join interactive reader");
    assert!(
        output.contains("PTY_INTERACTIVE_OK"),
        "expected interactive echo in output: {output:?}"
    );
    #[cfg(not(windows))]
    assert!(
        output.contains("40 120"),
        "expected resized TTY size `40 120` in output: {output:?}"
    );
}

#[test]
fn pty_terminate_kills_running_shell_and_closes_reader() {
    let cfg = interactive_shell_config(90, 30);
    let pty = PtyProcess::spawn(&cfg).expect("spawn long-lived PTY shell");
    assert!(
        pty.try_wait().expect("try_wait running shell").is_none(),
        "interactive shell should still be running"
    );

    pty.resize(100, 32).expect("resize running shell");

    let mut reader = pty.reader_clone().expect("clone reader before terminate");
    let reader_handle = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = reader.read_to_end(&mut buf);
        buf.len()
    });

    pty.terminate().expect("terminate running shell");
    let _read_bytes = reader_handle
        .join()
        .expect("reader hits EOF after terminate");
    assert!(
        pty.try_wait().expect("try_wait after terminate").is_some(),
        "terminated shell must record exit status"
    );
}
