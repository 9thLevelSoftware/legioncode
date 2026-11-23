//! Unix POSIX PTY (`posix_openpt` + `TIOCSWINSZ` + `setsid` + `TIOCSCTTY`) implementation.

use core::ffi::{CStr, c_char, c_int, c_ulong};
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::unix::io::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};

use crate::pty::{PtyError, PtySpawnConfig};

const O_RDWR: c_int = 2;
const F_SETFD: c_int = 2;
const FD_CLOEXEC: c_int = 1;
const SIGKILL: c_int = 9;

#[cfg(target_os = "macos")]
const O_NOCTTY: c_int = 0x0002_0000;
#[cfg(not(target_os = "macos"))]
const O_NOCTTY: c_int = 0x0000_0100;

#[cfg(target_os = "macos")]
const TIOCPTYGNAME: c_ulong = 0x4080_7453;

#[cfg(all(target_os = "linux", target_env = "musl"))]
const TIOCSWINSZ: c_int = 0x5414;
#[cfg(all(target_os = "linux", target_env = "musl"))]
const TIOCSCTTY: c_int = 0x540E;

#[cfg(all(
    not(target_os = "macos"),
    not(all(target_os = "linux", target_env = "musl"))
))]
const TIOCSWINSZ: c_ulong = 0x5414;
#[cfg(all(
    not(target_os = "macos"),
    not(all(target_os = "linux", target_env = "musl"))
))]
const TIOCSCTTY: c_ulong = 0x540E;

#[cfg(target_os = "macos")]
const TIOCSWINSZ: c_ulong = 0x8008_7467;
#[cfg(target_os = "macos")]
const TIOCSCTTY: c_ulong = 0x2000_7461;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Winsize {
    row: u16,
    col: u16,
    xpixel: u16,
    ypixel: u16,
}

// SAFETY: Standard POSIX libc entrypoints called only with valid file descriptors and buffers.
unsafe extern "C" {
    fn posix_openpt(oflag: c_int) -> c_int;
    fn grantpt(fildes: c_int) -> c_int;
    fn unlockpt(fildes: c_int) -> c_int;
    #[cfg(not(target_os = "macos"))]
    fn ptsname_r(fildes: c_int, buf: *mut c_char, buflen: usize) -> c_int;
    fn open(path: *const c_char, oflag: c_int, ...) -> c_int;
    fn fcntl(fd: c_int, cmd: c_int, ...) -> c_int;
    fn setsid() -> c_int;
    fn kill(pid: c_int, sig: c_int) -> c_int;
    #[cfg(all(target_os = "linux", target_env = "musl"))]
    fn ioctl(fd: c_int, request: c_int, ...) -> c_int;
    #[cfg(not(all(target_os = "linux", target_env = "musl")))]
    fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
}

fn poisoned() -> PtyError {
    PtyError::Io(io::Error::other("PTY synchronization mutex poisoned"))
}

fn set_cloexec(fd: RawFd) -> io::Result<()> {
    // SAFETY: `fd` is an open file descriptor and `F_SETFD`/`FD_CLOEXEC` take a single `c_int`.
    if unsafe { fcntl(fd, F_SETFD, FD_CLOEXEC) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn slave_tty_path(master_fd: RawFd) -> io::Result<PathBuf> {
    let mut buf = [0 as c_char; 128];
    #[cfg(target_os = "macos")]
    {
        // SAFETY: `master_fd` is a valid PTY master and `TIOCPTYGNAME` writes a NUL-terminated path into 128 bytes.
        if unsafe { ioctl(master_fd, TIOCPTYGNAME, buf.as_mut_ptr()) } == -1 {
            return Err(io::Error::last_os_error());
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        // SAFETY: `master_fd` is a valid PTY master and `buf` provides `buf.len()` writable bytes.
        let rc = unsafe { ptsname_r(master_fd, buf.as_mut_ptr(), buf.len()) };
        if rc != 0 {
            return Err(io::Error::from_raw_os_error(rc));
        }
    }
    // SAFETY: `TIOCPTYGNAME` / `ptsname_r` wrote a NUL-terminated C string into `buf`.
    let cstr = unsafe { CStr::from_ptr(buf.as_ptr()) };
    Ok(PathBuf::from(cstr.to_string_lossy().into_owned()))
}

fn open_master_and_slave(cols: u16, rows: u16) -> Result<(File, File), PtyError> {
    // SAFETY: Opening a fresh PTY master device with `O_RDWR | O_NOCTTY`.
    let raw_master = unsafe { posix_openpt(O_RDWR | O_NOCTTY) };
    if raw_master < 0 {
        return Err(PtyError::SpawnFailed(io::Error::last_os_error()));
    }
    // SAFETY: `raw_master` is a newly opened, uniquely owned file descriptor.
    let master_fd = unsafe { OwnedFd::from_raw_fd(raw_master) };
    set_cloexec(master_fd.as_raw_fd()).map_err(PtyError::SpawnFailed)?;

    // SAFETY: `master_fd` is a valid PTY master owned by this function.
    if unsafe { grantpt(master_fd.as_raw_fd()) } != 0 {
        return Err(PtyError::SpawnFailed(io::Error::last_os_error()));
    }
    // SAFETY: `master_fd` is a valid PTY master granted above.
    if unsafe { unlockpt(master_fd.as_raw_fd()) } != 0 {
        return Err(PtyError::SpawnFailed(io::Error::last_os_error()));
    }

    let slave_path = slave_tty_path(master_fd.as_raw_fd()).map_err(PtyError::SpawnFailed)?;
    let slave_cstr = std::ffi::CString::new(slave_path.as_os_str().as_encoded_bytes())
        .map_err(|e| PtyError::SpawnFailed(io::Error::new(io::ErrorKind::InvalidInput, e)))?;
    // SAFETY: `slave_cstr` is a valid NUL-terminated path returned by the kernel for this PTY.
    let raw_slave = unsafe { open(slave_cstr.as_ptr(), O_RDWR | O_NOCTTY) };
    if raw_slave < 0 {
        return Err(PtyError::SpawnFailed(io::Error::last_os_error()));
    }
    // SAFETY: `raw_slave` is a newly opened, uniquely owned file descriptor.
    let slave_fd = unsafe { OwnedFd::from_raw_fd(raw_slave) };
    set_cloexec(slave_fd.as_raw_fd()).map_err(PtyError::SpawnFailed)?;

    // Configure initial window size only after opening the slave device: XNU clears `TS_IOCTL_NOT_OK` in `ptsopen`.
    let ws = Winsize {
        row: rows,
        col: cols,
        xpixel: 0,
        ypixel: 0,
    };
    // SAFETY: `master_fd` and `slave_fd` are open and `TIOCSWINSZ` reads the initialized `Winsize` struct.
    if unsafe { ioctl(master_fd.as_raw_fd(), TIOCSWINSZ, &raw const ws) } == -1 {
        return Err(PtyError::SpawnFailed(io::Error::last_os_error()));
    }

    Ok((File::from(master_fd), File::from(slave_fd)))
}

#[derive(Debug)]
pub(crate) struct PlatformReader(File);

impl Read for PlatformReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.0.read(buf) {
            // Linux PTY masters yield EIO once all slave file descriptors close.
            #[cfg(target_os = "linux")]
            Err(err) if err.raw_os_error() == Some(5) => Ok(0),
            other => other,
        }
    }
}

#[derive(Debug)]
pub(crate) struct PlatformPty {
    pid: u32,
    master: Arc<File>,
    child: Mutex<Option<Child>>,
    exit_status: Mutex<Option<ExitStatus>>,
}

impl PlatformPty {
    pub(crate) fn spawn(config: &PtySpawnConfig) -> Result<Self, PtyError> {
        let (master, slave) = open_master_and_slave(config.cols, config.rows)?;
        let slave_in = slave.try_clone().map_err(PtyError::SpawnFailed)?;
        let slave_out = slave.try_clone().map_err(PtyError::SpawnFailed)?;
        let slave_err = slave;

        let mut command = Command::new(&config.program);
        command.args(&config.args);
        if let Some(cwd) = &config.cwd {
            command.current_dir(cwd);
        }
        config.env.apply_to_command(&mut command);
        command
            .stdin(Stdio::from(slave_in))
            .stdout(Stdio::from(slave_out))
            .stderr(Stdio::from(slave_err));

        // SAFETY: Post-fork pre-exec runs in the forked child before `exec`; calls async-signal-safe `setsid` and `ioctl(TIOCSCTTY)`.
        unsafe {
            command.pre_exec(|| {
                if setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                if ioctl(0, TIOCSCTTY, 0 as c_int) == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }

        let spawn_result = command.spawn();
        // Drop `command` immediately so parent releases its duplicated slave descriptors.
        drop(command);
        let child = spawn_result.map_err(PtyError::SpawnFailed)?;
        let pid = child.id();

        Ok(Self {
            pid,
            master: Arc::new(master),
            child: Mutex::new(Some(child)),
            exit_status: Mutex::new(None),
        })
    }

    pub(crate) fn reader_clone(&self) -> Result<PlatformReader, PtyError> {
        Ok(PlatformReader(self.master.try_clone()?))
    }

    pub(crate) fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        match (&*self.master).read(buf) {
            #[cfg(target_os = "linux")]
            Err(err) if err.raw_os_error() == Some(5) => Ok(0),
            other => other,
        }
    }

    pub(crate) fn write_all(&self, data: &[u8]) -> Result<(), PtyError> {
        let mut master = &*self.master;
        master.write_all(data)?;
        master.flush()?;
        Ok(())
    }

    pub(crate) fn resize(&self, cols: u16, rows: u16) -> Result<(), PtyError> {
        let ws = Winsize {
            row: rows,
            col: cols,
            xpixel: 0,
            ypixel: 0,
        };
        // SAFETY: `self.master` is a live PTY master and `ws` is an initialized `Winsize`.
        if unsafe { ioctl(self.master.as_raw_fd(), TIOCSWINSZ, &raw const ws) } == -1 {
            return Err(PtyError::Io(io::Error::last_os_error()));
        }
        Ok(())
    }

    pub(crate) fn try_wait(&self) -> Result<Option<ExitStatus>, PtyError> {
        if let Some(status) = *self.exit_status.lock().map_err(|_| poisoned())? {
            return Ok(Some(status));
        }
        let mut child_guard = self.child.lock().map_err(|_| poisoned())?;
        let Some(child) = child_guard.as_mut() else {
            return Ok(*self.exit_status.lock().map_err(|_| poisoned())?);
        };
        if let Some(status) = child.try_wait()? {
            *self.exit_status.lock().map_err(|_| poisoned())? = Some(status);
            *child_guard = None;
            return Ok(Some(status));
        }
        Ok(None)
    }

    pub(crate) fn wait(&self) -> Result<ExitStatus, PtyError> {
        if let Some(status) = *self.exit_status.lock().map_err(|_| poisoned())? {
            return Ok(status);
        }
        let mut child_guard = self.child.lock().map_err(|_| poisoned())?;
        let Some(mut child) = child_guard.take() else {
            return self
                .exit_status
                .lock()
                .map_err(|_| poisoned())?
                .ok_or_else(|| PtyError::Io(io::Error::other("PTY child already reaped")));
        };
        drop(child_guard);
        let status = child.wait()?;
        *self.exit_status.lock().map_err(|_| poisoned())? = Some(status);
        Ok(status)
    }

    pub(crate) const fn pid(&self) -> u32 {
        self.pid
    }

    pub(crate) fn terminate(&self) -> Result<(), PtyError> {
        if self.exit_status.lock().map_err(|_| poisoned())?.is_some() {
            return Ok(());
        }
        let mut child_guard = self.child.lock().map_err(|_| poisoned())?;
        if let Some(mut child) = child_guard.take() {
            if let Ok(pgid) = c_int::try_from(self.pid)
                && pgid > 0
            {
                // SAFETY: `child` has not been reaped yet (`setsid()` set `pgid == self.pid`), so `pgid` cannot be recycled.
                unsafe {
                    let _ = kill(-pgid, SIGKILL);
                }
            }
            let _ = child.kill();
            let status = child.wait()?;
            *self.exit_status.lock().map_err(|_| poisoned())? = Some(status);
        }
        Ok(())
    }
}

impl Drop for PlatformPty {
    fn drop(&mut self) {
        if let Ok(slot) = self.child.get_mut()
            && let Some(mut child) = slot.take()
        {
            if let Ok(pgid) = c_int::try_from(self.pid)
                && pgid > 0
            {
                // SAFETY: `child` is still un-reaped in `self.child`, so its session process group `pgid` is still reserved.
                unsafe {
                    let _ = kill(-pgid, SIGKILL);
                }
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
