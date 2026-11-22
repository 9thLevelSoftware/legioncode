//! Windows `ConPTY` (`CreatePseudoConsole` + `STARTUPINFOEXW`) backend for [`crate::pty::PtyProcess`].

use core::ffi::c_void;
use std::fs::File;
use std::io::{self, Read, Write};
use std::mem::{MaybeUninit, size_of};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::os::windows::process::ExitStatusExt;
use std::process::ExitStatus;
use std::ptr::{NonNull, null};
use std::sync::{Arc, Mutex};

use crate::pty::{PtyError, PtySpawnConfig};
use crate::pty_win32_ffi::{
    AttrList, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW, CreatePseudoConsole,
    EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess, Hpc, INFINITE, ProcessInformation,
    PseudoConsoleHandle, ResizePseudoConsole, ResumeThread, STARTF_USESTDHANDLES, StartupInfoExW,
    StartupInfoW, TerminateProcess, WAIT_OBJECT_0, WAIT_TIMEOUT, WaitForSingleObject,
    build_command_line, build_env_block, coord_from, create_pipe_pair,
};
use crate::win32_job::JobHandle;

fn poisoned() -> PtyError {
    PtyError::Io(io::Error::other("PTY synchronization mutex poisoned"))
}

#[derive(Debug)]
pub(crate) struct PlatformReader(File);

impl Read for PlatformReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

#[derive(Debug)]
pub(crate) struct PlatformPty {
    pid: u32,
    job: JobHandle,
    process_handle: OwnedHandle,
    console: Mutex<Option<PseudoConsoleHandle>>,
    writer: Mutex<Option<File>>,
    reader: Arc<File>,
    exit_status: Mutex<Option<ExitStatus>>,
}

impl PlatformPty {
    pub(crate) fn spawn(config: &PtySpawnConfig) -> Result<Self, PtyError> {
        let size = coord_from(config.cols, config.rows)?;
        let (in_read, in_write) = create_pipe_pair()?;
        let (out_read, out_write) = create_pipe_pair()?;

        let mut raw_pc: Hpc = std::ptr::null_mut();
        // SAFETY: `in_read` and `out_write` are valid live pipe handles and `raw_pc` receives HPCON.
        let hr = unsafe {
            CreatePseudoConsole(
                size,
                in_read.as_raw_handle(),
                out_write.as_raw_handle(),
                0,
                &raw mut raw_pc,
            )
        };
        if hr < 0 {
            return Err(PtyError::SpawnFailed(io::Error::from_raw_os_error(hr)));
        }
        let h_pc = NonNull::new(raw_pc).ok_or_else(|| {
            PtyError::SpawnFailed(io::Error::other("CreatePseudoConsole returned null HPCON"))
        })?;
        let pc_guard = PseudoConsoleHandle(h_pc);

        // Close our copies immediately so EOF reaches `out_read` when ConPTY shuts down.
        drop(in_read);
        drop(out_write);

        let attr_list = AttrList::new_pseudoconsole(&pc_guard)?;
        let mut si_ex = StartupInfoExW {
            startup_info: StartupInfoW {
                cb: u32::try_from(size_of::<StartupInfoExW>()).unwrap_or(0),
                dw_flags: STARTF_USESTDHANDLES,
                ..Default::default()
            },
            lp_attribute_list: attr_list.ptr,
        };

        let mut cmd_line = build_command_line(config)?;
        let env_block = build_env_block(config);
        let cwd_wide = encode_optional_cwd(config.cwd.as_deref())?;
        let cwd_ptr = cwd_wide.as_ref().map_or(null(), Vec::as_ptr);

        let mut pi = MaybeUninit::<ProcessInformation>::zeroed();
        let flags = EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT | CREATE_SUSPENDED;
        // SAFETY: All pointers reference live buffers initialized for the duration of `CreateProcessW`.
        let created = unsafe {
            CreateProcessW(
                null(),
                cmd_line.as_mut_ptr(),
                null(),
                null(),
                0,
                flags,
                env_block.as_ptr().cast::<c_void>(),
                cwd_ptr,
                &raw mut si_ex,
                pi.as_mut_ptr(),
            )
        };
        if created == 0 {
            return Err(PtyError::SpawnFailed(io::Error::last_os_error()));
        }
        // SAFETY: `CreateProcessW` returned non-zero, so `pi` holds valid handles and IDs.
        let pi = unsafe { pi.assume_init() };
        // SAFETY: `pi.h_process` and `pi.h_thread` are owned kernel handles from `CreateProcessW`.
        let (process_handle, thread_handle) = unsafe {
            (
                OwnedHandle::from_raw_handle(pi.h_process),
                OwnedHandle::from_raw_handle(pi.h_thread),
            )
        };
        drop(attr_list);

        let job = JobHandle::new_kill_on_close()?;
        if let Err(err) = job.assign_handle(&process_handle) {
            // SAFETY: `process_handle` is a live process handle created suspended.
            unsafe {
                let _ = TerminateProcess(process_handle.as_raw_handle(), 1);
            }
            return Err(err.into());
        }
        // SAFETY: `thread_handle` is the primary suspended thread from `CreateProcessW`.
        if unsafe { ResumeThread(thread_handle.as_raw_handle()) } == u32::MAX {
            let err = io::Error::last_os_error();
            let _ = job.terminate(1);
            return Err(PtyError::SpawnFailed(err));
        }
        drop(thread_handle);

        Ok(Self {
            pid: pi.dw_process_id,
            job,
            process_handle,
            console: Mutex::new(Some(pc_guard)),
            writer: Mutex::new(Some(File::from(in_write))),
            reader: Arc::new(File::from(out_read)),
            exit_status: Mutex::new(None),
        })
    }

    fn close_console_and_writer(&self) {
        if let Ok(mut w) = self.writer.lock() {
            let _ = w.take();
        }
        if let Ok(mut pc) = self.console.lock() {
            let _ = pc.take();
        }
    }

    pub(crate) fn reader_clone(&self) -> Result<PlatformReader, PtyError> {
        Ok(PlatformReader(self.reader.try_clone()?))
    }

    pub(crate) fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        (&*self.reader).read(buf)
    }

    pub(crate) fn write_all(&self, data: &[u8]) -> Result<(), PtyError> {
        let mut guard = self.writer.lock().map_err(|_| poisoned())?;
        let Some(writer) = guard.as_mut() else {
            return Err(PtyError::Io(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "PTY input stream is closed",
            )));
        };
        writer.write_all(data)?;
        writer.flush()?;
        Ok(())
    }

    pub(crate) fn resize(&self, cols: u16, rows: u16) -> Result<(), PtyError> {
        let size = coord_from(cols, rows)?;
        let guard = self.console.lock().map_err(|_| poisoned())?;
        let Some(pc) = guard.as_ref() else {
            return Err(PtyError::Io(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "PTY is already closed",
            )));
        };
        // SAFETY: `pc` is a live HPCON handle held under `self.console`.
        let hr = unsafe { ResizePseudoConsole(pc.as_ptr(), size) };
        if hr < 0 {
            return Err(PtyError::Io(io::Error::from_raw_os_error(hr)));
        }
        Ok(())
    }

    fn read_exit_code(&self) -> Result<ExitStatus, PtyError> {
        let mut code = 0_u32;
        // SAFETY: `self.process_handle` is live and `code` is a valid stack `u32`.
        if unsafe { GetExitCodeProcess(self.process_handle.as_raw_handle(), &raw mut code) } == 0 {
            return Err(PtyError::Io(io::Error::last_os_error()));
        }
        let status = ExitStatus::from_raw(code);
        *self.exit_status.lock().map_err(|_| poisoned())? = Some(status);
        self.close_console_and_writer();
        Ok(status)
    }

    pub(crate) fn try_wait(&self) -> Result<Option<ExitStatus>, PtyError> {
        if let Some(status) = *self.exit_status.lock().map_err(|_| poisoned())? {
            return Ok(Some(status));
        }
        // SAFETY: `self.process_handle` remains open for `&self`.
        match unsafe { WaitForSingleObject(self.process_handle.as_raw_handle(), 0) } {
            WAIT_OBJECT_0 => self.read_exit_code().map(Some),
            WAIT_TIMEOUT => Ok(None),
            _ => Err(PtyError::Io(io::Error::last_os_error())),
        }
    }

    pub(crate) fn wait(&self) -> Result<ExitStatus, PtyError> {
        if let Some(status) = *self.exit_status.lock().map_err(|_| poisoned())? {
            return Ok(status);
        }
        // SAFETY: `self.process_handle` remains open for `&self`.
        if unsafe { WaitForSingleObject(self.process_handle.as_raw_handle(), INFINITE) }
            != WAIT_OBJECT_0
        {
            return Err(PtyError::Io(io::Error::last_os_error()));
        }
        self.read_exit_code()
    }

    pub(crate) const fn pid(&self) -> u32 {
        self.pid
    }

    pub(crate) fn terminate(&self) -> Result<(), PtyError> {
        let already_exited = self.exit_status.lock().map_err(|_| poisoned())?.is_some();
        if already_exited {
            self.close_console_and_writer();
            return Ok(());
        }
        let term_res = self.job.terminate(1);
        self.close_console_and_writer();
        term_res?;
        // SAFETY: `self.process_handle` is live; capture exit status if the child finished quickly.
        if unsafe { WaitForSingleObject(self.process_handle.as_raw_handle(), 2_000) }
            == WAIT_OBJECT_0
        {
            let _ = self.read_exit_code();
        }
        Ok(())
    }
}

impl Drop for PlatformPty {
    fn drop(&mut self) {
        let still_running = self
            .exit_status
            .get_mut()
            .ok()
            .and_then(|slot| *slot)
            .is_none();
        if still_running {
            let _ = self.job.terminate(1);
        }
        self.close_console_and_writer();
    }
}

fn encode_optional_cwd(cwd: Option<&std::path::Path>) -> Result<Option<Vec<u16>>, PtyError> {
    cwd.map(|p| {
        let mut wide: Vec<u16> = p.as_os_str().encode_wide().collect();
        if wide.contains(&0) {
            return Err(PtyError::SpawnFailed(io::Error::new(
                io::ErrorKind::InvalidInput,
                "working directory contains interior NUL",
            )));
        }
        wide.push(0);
        Ok(wide)
    })
    .transpose()
}
