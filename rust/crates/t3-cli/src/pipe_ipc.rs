//! Shared synchronous Named Pipe / Unix Domain Socket single-line JSON exchange.

use std::io::{ErrorKind, Read, Write};
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt as _;
#[cfg(unix)]
use std::os::unix::net::UnixStream;
#[cfg(unix)]
use std::path::Path;

#[cfg(windows)]
const BUSY_RETRY_SLEEP: Duration = Duration::from_millis(10);
#[cfg(windows)]
const WIN32_ERROR_PIPE_BUSY: i32 = 231;

/// Failure mode while exchanging a single newline-delimited JSON frame over local IPC.
#[derive(Debug)]
pub(crate) enum PipeExchangeError {
    /// Target pipe/socket is missing or could not be connected cleanly.
    Unreachable,
    /// Connected peer failed protocol framing or dropped the connection before responding.
    Protocol,
}

/// Exchanging `request_line` (without trailing `\n`) with `address` over Windows Named Pipe or Unix socket.
pub(crate) fn exchange_json_line(
    address: &str,
    request_line: &str,
    max_req_bytes: usize,
    max_resp_bytes: usize,
    timeout: Duration,
) -> Result<String, PipeExchangeError> {
    if request_line.len() + 1 > max_req_bytes {
        return Err(PipeExchangeError::Protocol);
    }

    #[cfg(windows)]
    {
        exchange_windows_pipe(address, request_line, max_resp_bytes, timeout)
    }
    #[cfg(unix)]
    {
        exchange_unix_socket(address, request_line, max_resp_bytes, timeout)
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = (address, max_resp_bytes, timeout);
        Err(PipeExchangeError::Unreachable)
    }
}

#[cfg(windows)]
fn exchange_windows_pipe(
    address: &str,
    request_line: &str,
    max_resp_bytes: usize,
    timeout: Duration,
) -> Result<String, PipeExchangeError> {
    let deadline = Instant::now() + timeout;
    let mut pipe = loop {
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(address)
        {
            Ok(file) => break file,
            Err(err)
                if err.raw_os_error() == Some(WIN32_ERROR_PIPE_BUSY)
                    && Instant::now() < deadline =>
            {
                std::thread::sleep(BUSY_RETRY_SLEEP);
            }
            Err(_) => {
                return Err(PipeExchangeError::Unreachable);
            }
        }
    };

    let mut frame = Vec::with_capacity(request_line.len() + 1);
    frame.extend_from_slice(request_line.as_bytes());
    frame.push(b'\n');
    pipe.write_all(&frame)
        .and_then(|()| pipe.flush())
        .map_err(|_| PipeExchangeError::Protocol)?;

    read_bounded_line(&mut pipe, max_resp_bytes)
}

#[cfg(unix)]
fn exchange_unix_socket(
    address: &str,
    request_line: &str,
    max_resp_bytes: usize,
    timeout: Duration,
) -> Result<String, PipeExchangeError> {
    let mut stream = UnixStream::connect(address).map_err(|_| PipeExchangeError::Unreachable)?;
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));

    let mut frame = Vec::with_capacity(request_line.len() + 1);
    frame.extend_from_slice(request_line.as_bytes());
    frame.push(b'\n');
    stream
        .write_all(&frame)
        .and_then(|()| stream.flush())
        .map_err(|_| PipeExchangeError::Protocol)?;

    read_bounded_line(&mut stream, max_resp_bytes)
}

fn read_bounded_line(
    reader: &mut impl Read,
    max_resp_bytes: usize,
) -> Result<String, PipeExchangeError> {
    let mut bytes = Vec::new();
    let mut buf = [0u8; 4096];
    let mut newline_idx = None;

    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if bytes.len() + n > max_resp_bytes {
                    return Err(PipeExchangeError::Protocol);
                }
                let start = bytes.len();
                bytes.extend_from_slice(&buf[..n]);
                if let Some(pos) = bytes[start..].iter().position(|&b| b == b'\n') {
                    let abs = start + pos;
                    if abs != bytes.len() - 1 {
                        return Err(PipeExchangeError::Protocol);
                    }
                    newline_idx = Some(abs);
                    break;
                }
            }
            Err(err) if err.kind() == ErrorKind::Interrupted => {}
            Err(_) => return Err(PipeExchangeError::Protocol),
        }
    }

    let Some(end) = newline_idx else {
        return Err(PipeExchangeError::Protocol);
    };

    String::from_utf8(bytes[..end].to_vec()).map_err(|_| PipeExchangeError::Protocol)
}

/// Returns the current OS user ID on Unix, or `None` on non-Unix platforms.
pub(crate) fn current_unix_uid() -> Option<u32> {
    #[cfg(unix)]
    {
        if let Ok(meta) = std::fs::metadata("/proc/self") {
            return Some(meta.uid());
        }
        let out = std::process::Command::new("id").arg("-u").output().ok()?;
        out.status
            .success()
            .then(|| {
                String::from_utf8_lossy(&out.stdout)
                    .trim()
                    .parse::<u32>()
                    .ok()
            })
            .flatten()
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// Verifies a Unix desktop socket directory exists, is not a symlink, is owner-only (`mode & 0o077 == 0`),
/// and matches `expected_uid` (`app.test.ts:242-286`).
#[cfg(unix)]
pub(crate) fn validate_unix_socket_dir(dir: &Path, expected_uid: Option<u32>) -> bool {
    let Some(uid) = expected_uid else {
        return false;
    };
    let Ok(meta) = std::fs::symlink_metadata(dir) else {
        return false;
    };
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return false;
    }
    meta.uid() == uid && (meta.mode() & 0o077) == 0
}
